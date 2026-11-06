use axum::{
    extract::{Query, Request, State},
    http::{header, StatusCode},
    middleware::Next,
    response::{Html, IntoResponse, Json, Redirect, Response},
    routing::{get, post},
    Router,
};
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tower_http::services::ServeFile;
use tower::ServiceExt;

use crate::auth::{AclEngine, AclRule, AuthManager, Claims};
use crate::config::HematiteConfig;
use crate::crawler::Crawler;
use crate::template::TemplateEngine;

#[derive(Clone)]
pub struct AppState {
    pub db_pool: Pool<SqliteConnectionManager>,
    pub auth_manager: Arc<AuthManager>,
    pub template_engine: Arc<TemplateEngine>,
    pub crawler: Arc<Crawler>,
    pub initialized: Arc<AtomicBool>,
    pub content_root: std::path::PathBuf,
    pub config: Arc<HematiteConfig>,
    pub theme_dir: std::path::PathBuf,
}

pub fn create_router(state: AppState) -> Router {
    Router::new()
        // API routes
        .route("/_api/v1/page", get(api_page))
        .route("/_api/v1/search", get(api_search))
        .route("/_search", get(search_page))
        .route("/_resolve", get(resolve_wikilink))
        .route("/_media/*path", get(serve_media))
        .route("/_static/*path", get(serve_static))
        // Auth routes
        .route("/_user/login", get(login_page).post(login_action))
        .route("/_user/logout", get(logout_action))
        // Admin routes
        .route("/_admin/", get(admin_page))
        .route("/_admin/users/create", post(admin_create_user))
        .route("/_admin/users/delete", post(admin_delete_user))
        .route("/_admin/users/bump_version", post(admin_bump_version))
        .route("/_admin/acls/create", post(admin_create_acl))
        .route("/_admin/acls/delete", post(admin_delete_acl))
        // Wildcard SSR route
        .fallback(ssr_handler)
        // ACL middleware applied across requests
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            verify_acl,
        ))
        .with_state(state)
}

// -------------------------------------------------------------
// Middleware: verify_acl
// -------------------------------------------------------------
pub async fn verify_acl(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let path = req.uri().path().to_string();

    // 1. Bypass public namespaces & internal routes that handle their own auth
    if path == "/_user/login"
        || path == "/_user/logout"
        || path.starts_with("/_static/")
        || path.starts_with("/static/")
        || path.starts_with("/_media/")
        || path == "/favicon.ico"
    {
        return Ok(next.run(req).await);
    }

    // 2. Extract token from cookie or Authorization header
    let claims = extract_claims_from_request(&req, &state.auth_manager);

    // 3. Admin routes require admin role & valid token version check
    if path.starts_with("/_admin") {
        match claims {
            Some(ref c) if c.role == "admin" => {
                if !verify_token_version(&state, &c.username, c.ver) {
                    return Ok(Redirect::to("/_user/login").into_response());
                }
                return Ok(next.run(req).await);
            }
            Some(_) => {
                let is_spa = path == "/_api/v1/page";
                return Ok(render_status_response(&state, StatusCode::FORBIDDEN, &path, "You do not have administrative privileges to access this area.", is_spa));
            }
            None => {
                let redirect_url = format!("/_user/login?redirect={}", urlencoding::encode(&path));
                return Ok(Redirect::to(&redirect_url).into_response());
            }
        }
    }

    // Determine target path for ACL check (for /_api/v1/page, evaluate target path param)
    let acl_target_path = if path == "/_api/v1/page" {
        req.uri()
            .query()
            .and_then(|q| {
                q.split('&')
                    .find(|p| p.starts_with("path="))
                    .and_then(|p| p.split_once('='))
                    .map(|(_, v)| urlencoding::decode(v).unwrap_or_default().to_string())
            })
            .unwrap_or(path.clone())
    } else {
        path.clone()
    };

    let (is_anon, rules) = match claims {
        Some(ref c) => {
            // Verify token version in database
            if !verify_token_version(&state, &c.username, c.ver) {
                // Version mismatch! Invalidate and redirect to login
                return Ok(Redirect::to("/_user/login").into_response());
            }
            (false, c.acl.clone())
        }
        None => {
            // Anonymous rules from database (rules assigned to '*' or anonymous)
            let default_rules = get_anonymous_rules(&state);
            (true, default_rules)
        }
    };

    if !AclEngine::check_permission(&acl_target_path, &rules) {
        if is_anon {
            let redirect_url = format!("/_user/login?redirect={}", urlencoding::encode(&path));
            return Ok(Redirect::to(&redirect_url).into_response());
        }
        let is_spa = path == "/_api/v1/page";
        return Ok(render_status_response(&state, StatusCode::FORBIDDEN, &acl_target_path, "You do not have permission to view this resource.", is_spa));
    }

    Ok(next.run(req).await)
}

#[derive(Debug, Clone, Serialize)]
pub struct UserContext {
    pub is_authenticated: bool,
    pub username: String,
    pub role: String,
    pub is_admin: bool,
}

fn build_user_context(claims: Option<&Claims>) -> UserContext {
    match claims {
        Some(c) => UserContext {
            is_authenticated: true,
            username: c.username.clone(),
            role: c.role.clone(),
            is_admin: c.role == "admin",
        },
        None => UserContext {
            is_authenticated: false,
            username: "Mysterious Wanderer".to_string(),
            role: "anonymous".to_string(),
            is_admin: false,
        },
    }
}

fn extract_claims_from_request(req: &Request, auth: &AuthManager) -> Option<Claims> {
    // Check Authorization: Bearer <token>
    if let Some(auth_header) = req.headers().get(header::AUTHORIZATION) {
        if let Ok(val) = auth_header.to_str() {
            if let Some(token) = val.strip_prefix("Bearer ") {
                if let Ok(claims) = auth.verify_jwt(token) {
                    return Some(claims);
                }
            }
        }
    }

    // Check Cookie: jwt=<token>
    if let Some(cookie_header) = req.headers().get(header::COOKIE) {
        if let Ok(cookie_str) = cookie_header.to_str() {
            for part in cookie_str.split(';') {
                let trimmed = part.trim();
                if let Some(token) = trimmed.strip_prefix("jwt=") {
                    if let Ok(claims) = auth.verify_jwt(token) {
                        return Some(claims);
                    }
                }
            }
        }
    }

    None
}

fn verify_token_version(state: &AppState, username: &str, token_version: i32) -> bool {
    let conn = match state.db_pool.get() {
        Ok(c) => c,
        Err(_) => return false,
    };

    let db_version: Option<i32> = conn
        .query_row(
            "SELECT token_version FROM users WHERE username = ?1;",
            params![username],
            |r| r.get(0),
        )
        .ok();

    match db_version {
        Some(v) => v == token_version,
        None => false,
    }
}

fn get_anonymous_rules(state: &AppState) -> Vec<AclRule> {
    let mut rules = Vec::new();
    if let Ok(conn) = state.db_pool.get() {
        if let Ok(mut stmt) = conn.prepare(
            "SELECT verb, path_pattern FROM acls WHERE username = '*' OR username = 'anonymous' ORDER BY priority ASC, id ASC;",
        ) {
            let rows = stmt.query_map([], |r| {
                Ok(AclRule {
                    v: r.get(0)?,
                    p: r.get(1)?,
                })
            });
            if let Ok(mapped) = rows {
                for r in mapped.flatten() {
                    rules.push(r);
                }
            }
        }
    }
    rules
}

// -------------------------------------------------------------
// SSR & Page Serving Handler
// -------------------------------------------------------------
async fn ssr_handler(
    State(state): State<AppState>,
    req: Request,
) -> Result<Response, (StatusCode, String)> {
    let path = req.uri().path().to_string();
    let normalized = normalize_url_path(&path);

    // Boot-time lock fallback: if requested path not in SQLite and not initialized
    ensure_file_indexed(&state, &normalized).await;

    let conn = state.db_pool.get().map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Database error: {}", e),
        )
    })?;

    // 1. Check if direct page exists
    let page: Option<(String, String, String, String)> = conn
        .query_row(
            "SELECT title, frontmatter_json, parsed_html, parent_path FROM pages WHERE url_path = ?1;",
            params![normalized],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .ok();

    let claims = extract_claims_from_request(&req, &state.auth_manager);
    let user_ctx = build_user_context(claims.as_ref());
    let all_tags = get_all_distinct_tags(&conn);

    if let Some((title, fm_json, html, parent_path)) = page {
        let (nav_html, breadcrumbs_html, toc_html) = build_page_chrome(&conn, &state.template_engine, &normalized, &title);
        let fm_val: serde_json::Value =
            serde_json::from_str(&fm_json).unwrap_or(json!({}));
        let title_source = fm_val.get("title_source").and_then(|v| v.as_str()).unwrap_or("frontmatter").to_string();
        let (prev_sibling_url, next_sibling_url) = get_page_siblings(&conn, &parent_path, &normalized);
        let page_tags = extract_tags_from_fm(&fm_val);

        let rendered = state
            .template_engine
            .render(
                "page.html",
                json!({
                    "title": title,
                    "title_source": title_source,
                    "content_html": html,
                    "navigation_html": nav_html,
                    "breadcrumbs_html": breadcrumbs_html,
                    "toc_html": toc_html,
                    "tags": page_tags,
                    "all_tags": all_tags,
                    "prev_sibling_url": prev_sibling_url,
                    "next_sibling_url": next_sibling_url,
                    "config": state.config.to_json_value(),
                    "user": user_ctx,
                    "is_search_page": false,
                }),
            )
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Template error: {}", e),
                )
            })?;

        return Ok(Html(rendered).into_response());
    }

    // 2. Check if virtual directory
    let is_dir: bool = conn
        .query_row(
            "SELECT 1 FROM directories WHERE url_path = ?1;",
            params![normalized],
            |_| Ok(true),
        )
        .unwrap_or(false);

    if is_dir {
        // Query child pages and subdirectories
        let mut child_pages = Vec::new();
        let mut stmt = conn
            .prepare("SELECT url_path, title FROM pages WHERE parent_path = ?1 ORDER BY title ASC;")
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let p_rows = stmt
            .query_map(params![normalized], |r| {
                Ok(json!({
                    "url_path": r.get::<_, String>(0)?,
                    "title": r.get::<_, String>(1)?,
                }))
            })
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        for p in p_rows.flatten() {
            child_pages.push(p);
        }

        let mut subdirs = Vec::new();
        let mut d_stmt = conn
            .prepare(
                "SELECT d.url_path, 
                        COALESCE(d.title, p.title, REPLACE(SUBSTR(d.url_path, INSTR(d.url_path, '/') + 1), '-', ' ')),
                        CASE WHEN p.url_path IS NOT NULL THEN 1 ELSE 0 END
                 FROM directories d
                 LEFT JOIN pages p ON p.url_path = d.url_path AND p.is_dir_index = 1
                 WHERE d.parent_path = ?1 ORDER BY d.url_path ASC;",
            )
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let d_rows = d_stmt
            .query_map(params![normalized], |r| {
                let u: String = r.get(0)?;
                let title: String = r.get(1)?;
                let has_index: bool = r.get::<_, i32>(2)? == 1;
                let name = if has_index {
                    title
                } else {
                    u.split('/').last().unwrap_or(&u).replace('-', " ")
                };
                Ok(json!({
                    "url_path": u,
                    "name": name,
                    "has_index": has_index,
                }))
            })
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        for d in d_rows.flatten() {
            subdirs.push(d);
        }

        let dir_name = if normalized == "/" {
            "Home".to_string()
        } else {
            normalized.split('/').last().unwrap_or("Index").to_string()
        };

        let (nav_html, breadcrumbs_html, _) = build_page_chrome(&conn, &state.template_engine, &normalized, &dir_name);
        let rendered = state
            .template_engine
            .render(
                "directory_index.html",
                json!({
                    "title": format!("Directory: {}", dir_name),
                    "title_source": "filename",
                    "child_pages": child_pages,
                    "subdirectories": subdirs,
                    "navigation_html": nav_html,
                    "breadcrumbs_html": breadcrumbs_html,
                    "toc_html": "",
                    "tags": [],
                    "all_tags": all_tags,
                    "prev_sibling_url": null,
                    "next_sibling_url": null,
                    "config": state.config.to_json_value(),
                    "user": user_ctx,
                    "is_search_page": false,
                }),
            )
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Template error: {}", e),
                )
            })?;

        return Ok(Html(rendered).into_response());
    }

    Ok(render_status_response(&state, StatusCode::NOT_FOUND, &path, "We looked everywhere, but couldn't find the page you're looking for.", false))
}

// -------------------------------------------------------------
// SPA Region Endpoint: GET /_api/v1/page?path=...
// -------------------------------------------------------------
#[derive(Deserialize)]
struct PageQuery {
    path: String,
}

async fn api_page(
    State(state): State<AppState>,
    Query(q): Query<PageQuery>,
) -> Result<Response, (StatusCode, String)> {
    let normalized = normalize_url_path(&q.path);
    ensure_file_indexed(&state, &normalized).await;

    let conn = state.db_pool.get().map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Database error: {}", e),
        )
    })?;

    let page: Option<(String, String, String, String)> = conn
        .query_row(
            "SELECT title, frontmatter_json, parsed_html, parent_path FROM pages WHERE url_path = ?1;",
            params![normalized],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .ok();

    if let Some((title, fm_json, html, parent_path)) = page {
        let (nav_html, breadcrumbs_html, toc_html) = build_page_chrome(&conn, &state.template_engine, &normalized, &title);
        let fm_val: serde_json::Value =
            serde_json::from_str(&fm_json).unwrap_or(json!({}));
        let title_source = fm_val.get("title_source").and_then(|v| v.as_str()).unwrap_or("frontmatter").to_string();
        let (prev_sibling_url, next_sibling_url) = get_page_siblings(&conn, &parent_path, &normalized);
        let page_tags = extract_tags_from_fm(&fm_val);

        let resp_json = json!({
            "title": title,
            "title_source": title_source,
            "url": normalized,
            "frontmatter": fm_val,
            "tags": page_tags,
            "prev_sibling_url": prev_sibling_url,
            "next_sibling_url": next_sibling_url,
            "config": state.config.to_json_value(),
            "regions": {
                "content": html,
                "toc": toc_html,
                "breadcrumbs": breadcrumbs_html,
                "navigation": nav_html,
            }
        });

        return Ok(Json(resp_json).into_response());
    }

    // If directory
    let is_dir: bool = conn
        .query_row(
            "SELECT 1 FROM directories WHERE url_path = ?1;",
            params![normalized],
            |_| Ok(true),
        )
        .unwrap_or(false);

    if is_dir {
        let mut child_pages = Vec::new();
        let mut stmt = conn
            .prepare("SELECT url_path, title FROM pages WHERE parent_path = ?1 ORDER BY title ASC;")
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let p_rows = stmt
            .query_map(params![normalized], |r| {
                Ok(json!({
                    "url_path": r.get::<_, String>(0)?,
                    "title": r.get::<_, String>(1)?,
                }))
            })
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        for p in p_rows.flatten() {
            child_pages.push(p);
        }

        let mut subdirs = Vec::new();
        let mut d_stmt = conn
            .prepare(
                "SELECT d.url_path, 
                        COALESCE(d.title, p.title, REPLACE(SUBSTR(d.url_path, INSTR(d.url_path, '/') + 1), '-', ' ')),
                        CASE WHEN p.url_path IS NOT NULL THEN 1 ELSE 0 END
                 FROM directories d
                 LEFT JOIN pages p ON p.url_path = d.url_path AND p.is_dir_index = 1
                 WHERE d.parent_path = ?1 ORDER BY d.url_path ASC;",
            )
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let d_rows = d_stmt
            .query_map(params![normalized], |r| {
                let u: String = r.get(0)?;
                let title: String = r.get(1)?;
                let has_index: bool = r.get::<_, i32>(2)? == 1;
                let name = if has_index {
                    title
                } else {
                    u.split('/').last().unwrap_or(&u).replace('-', " ")
                };
                Ok(json!({
                    "url_path": u,
                    "name": name,
                    "has_index": has_index,
                }))
            })
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        for d in d_rows.flatten() {
            subdirs.push(d);
        }

        let dir_name = if normalized == "/" {
            "Home".to_string()
        } else {
            normalized.split('/').last().unwrap_or("Index").to_string()
        };
        let (nav_html, breadcrumbs_html, _) = build_page_chrome(&conn, &state.template_engine, &normalized, &dir_name);

        let content_html = state
            .template_engine
            .render(
                "components/directory_content.html",
                json!({
                    "dir_name": dir_name,
                    "subdirectories": subdirs,
                    "child_pages": child_pages,
                    "config": state.config.to_json_value(),
                }),
            )
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        let resp_json = json!({
            "title": format!("Directory: {}", dir_name),
            "url": normalized,
            "frontmatter": {},
            "config": state.config.to_json_value(),
            "regions": {
                "content": content_html,
                "toc": "",
                "breadcrumbs": breadcrumbs_html,
                "navigation": nav_html,
            }
        });
        return Ok(Json(resp_json).into_response());
    }

    Ok(render_status_response(&state, StatusCode::NOT_FOUND, &q.path, "We looked everywhere, but couldn't find the page you're looking for.", true))
}

// -------------------------------------------------------------
// FTS5 Search Endpoint: GET /_api/v1/search?q=...
// -------------------------------------------------------------
#[derive(Deserialize)]
struct SearchQuery {
    q: String,
}

#[derive(Serialize)]
struct SearchHit {
    url: String,
    title: String,
    snippet: String,
}

async fn api_search(
    State(state): State<AppState>,
    Query(q): Query<SearchQuery>,
) -> Result<Json<Vec<SearchHit>>, (StatusCode, String)> {
    let conn = state.db_pool.get().map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Database error: {}", e),
        )
    })?;

    let cleaned = q.q.replace('"', "").replace('*', "");
    let fts_query = format!("{}*", cleaned.trim());

    let mut stmt = conn
        .prepare("SELECT url_path, title, snippet(pages_fts, 2, '<b>', '</b>', '...', 15) FROM pages_fts WHERE pages_fts MATCH ?1 LIMIT 20;")
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let rows = stmt
        .query_map(params![fts_query], |row| {
            Ok(SearchHit {
                url: row.get(0)?,
                title: row.get(1)?,
                snippet: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
            })
        })
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut hits = Vec::new();
    for r in rows.flatten() {
        hits.push(r);
    }

    Ok(Json(hits))
}

async fn search_page(
    State(state): State<AppState>,
    Query(q): Query<SearchQuery>,
    req: Request,
) -> Result<Response, (StatusCode, String)> {
    let conn = state.db_pool.get().map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Database error: {}", e),
        )
    })?;

    let claims = extract_claims_from_request(&req, &state.auth_manager);
    let user_ctx = build_user_context(claims.as_ref());

    let cleaned = q.q.replace('"', "").replace('*', "");
    let fts_query = format!("{}*", cleaned.trim());

    let mut hits = Vec::new();
    if !cleaned.trim().is_empty() {
        if let Ok(mut stmt) = conn.prepare("SELECT url_path, title, snippet(pages_fts, 2, '<b>', '</b>', '...', 25) FROM pages_fts WHERE pages_fts MATCH ?1 LIMIT 50;") {
            if let Ok(rows) = stmt.query_map(params![fts_query], |row| {
                Ok(SearchHit {
                    url: row.get(0)?,
                    title: row.get(1)?,
                    snippet: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                })
            }) {
                for r in rows.flatten() {
                    hits.push(r);
                }
            }
        }
    }

    let page_title = format!("Search: {}", q.q);
    let (nav_html, _, _) = build_page_chrome(&conn, &state.template_engine, "/_search", &page_title);

    let search_content = state
        .template_engine
        .render(
            "search.html",
            json!({
                "query": q.q,
                "hits": hits,
                "config": state.config.to_json_value(),
            }),
        )
        .or_else(|_| {
            let mut out = format!("<h1>Search Results: {}</h1>", q.q);
            if hits.is_empty() {
                out.push_str("<p>No results found.</p>");
            } else {
                out.push_str("<ul class=\"search-page-results\">");
                for h in &hits {
                    out.push_str(&format!("<li><a href=\"{}\"><strong>{}</strong></a><p>{}</p></li>", h.url, h.title, h.snippet));
                }
                out.push_str("</ul>");
            }
            Ok::<_, minijinja::Error>(out)
        })
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let rendered = state
        .template_engine
        .render(
            "page.html",
            json!({
                "title": page_title,
                "content_html": search_content,
                "navigation_html": nav_html,
                "breadcrumbs_html": "", // Not shown on search result page
                "toc_html": "",
                "tags": [], // Not shown on search result page
                "title_source": "h1", // Prevents duplicate title header
                "is_search_page": true,
                "config": state.config.to_json_value(),
                "user": user_ctx,
            }),
        )
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Template error: {}", e)))?;

    Ok(Html(rendered).into_response())
}

// -------------------------------------------------------------
// Wikilink Target Resolver: GET /_resolve?target=...
// -------------------------------------------------------------
#[derive(Deserialize)]
struct ResolveQuery {
    target: String,
}

async fn resolve_wikilink(
    State(state): State<AppState>,
    Query(q): Query<ResolveQuery>,
) -> Result<Response, StatusCode> {
    let conn = state
        .db_pool
        .get()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Look in page_links or pages table directly
    let resolved: Option<String> = conn
        .query_row(
            "SELECT resolved_url FROM page_links WHERE target_raw = ?1 AND resolved_url IS NOT NULL LIMIT 1;",
            params![q.target],
            |r| r.get(0),
        )
        .ok();

    if let Some(target_url) = resolved {
        return Ok(Redirect::temporary(&target_url).into_response());
    }

    // Direct match against url_path or filename
    let clean_target = q.target
        .strip_suffix(".md")
        .or_else(|| q.target.strip_suffix(".markdown"))
        .unwrap_or(&q.target);

    let target_clean_path = if clean_target.starts_with('/') {
        clean_target.to_string()
    } else {
        format!("/{}", clean_target)
    };

    let direct: Option<String> = conn
        .query_row(
            "SELECT url_path FROM pages WHERE url_path = ?1 LIMIT 1;",
            params![target_clean_path],
            |r| r.get(0),
        )
        .ok();

    if let Some(target_url) = direct {
        return Ok(Redirect::temporary(&target_url).into_response());
    }

    // Match by filename / file_path in pages table
    let target_filename = q.target.split('/').last().unwrap_or(&q.target);
    let target_stem = target_filename
        .strip_suffix(".md")
        .or_else(|| target_filename.strip_suffix(".markdown"))
        .unwrap_or(target_filename);
    let file_pattern = format!("%/{}", target_filename);
    let file_pattern_md = format!("%/{}.md", target_stem);

    let by_file: Option<String> = conn
        .query_row(
            "SELECT url_path FROM pages WHERE file_path LIKE ?1 OR file_path LIKE ?2 ORDER BY length(url_path) ASC LIMIT 1;",
            params![file_pattern, file_pattern_md],
            |r| r.get(0),
        )
        .ok();

    if let Some(target_url) = by_file {
        return Ok(Redirect::temporary(&target_url).into_response());
    }

    Ok(Redirect::temporary(&format!("/{}", q.target.trim_start_matches('/'))).into_response())
}

// -------------------------------------------------------------
// Media Serving Handler: GET /_media/*path
// -------------------------------------------------------------
async fn serve_media(
    State(state): State<AppState>,
    req: Request,
) -> Result<Response, StatusCode> {
    use tower::ServiceExt;
    use tower_http::services::ServeFile;

    let path = req.uri().path();
    let raw_media_path = match path.strip_prefix("/_media/") {
        Some(p) => p,
        None => return Err(StatusCode::NOT_FOUND),
    };

    let decoded_path = urlencoding::decode(raw_media_path)
        .map_err(|_| StatusCode::BAD_REQUEST)?
        .to_string();

    // Prevent directory traversal attacks
    if decoded_path.contains("..") {
        return Err(StatusCode::FORBIDDEN);
    }

    // 1. Direct path check relative to content_root
    let candidate = state.content_root.join(&decoded_path);
    if candidate.is_file() {
        let serve_file = ServeFile::new(candidate);
        return match serve_file.oneshot(req).await {
            Ok(res) => Ok(res.into_response()),
            Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
        };
    }

    // 2. Search anywhere in content_root by filename or suffix match
    let target_filename = std::path::Path::new(&decoded_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&decoded_path)
        .to_lowercase();

    for entry in walkdir::WalkDir::new(&state.content_root).into_iter().flatten() {
        if entry.file_type().is_file() {
            let p = entry.path();
            if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                if name.to_lowercase() == target_filename {
                    let serve_file = ServeFile::new(p);
                    return match serve_file.oneshot(req).await {
                        Ok(res) => Ok(res.into_response()),
                        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
                    };
                }
            }
        }
    }

    Err(StatusCode::NOT_FOUND)
}

// -------------------------------------------------------------
// Static Asset Handler: GET /_static/*path
// -------------------------------------------------------------
async fn serve_static(
    State(state): State<AppState>,
    req: Request,
) -> Result<Response, StatusCode> {
    let path = req.uri().path();
    let static_path = match path.strip_prefix("/_static/") {
        Some(p) => p,
        None => return Err(StatusCode::NOT_FOUND),
    };

    // 1. Check if theme has an override file in <theme_dir>/static/<static_path>
    let theme_file = state.theme_dir.join("static").join(static_path);
    if theme_file.is_file() {
        let serve_file = ServeFile::new(theme_file);
        return match serve_file.oneshot(req).await {
            Ok(res) => Ok(res.into_response()),
            Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
        };
    }

    // 2. Default static assets on filesystem or embedded fallback
    let default_file = std::path::PathBuf::from("static").join(static_path);
    if default_file.is_file() {
        let serve_file = ServeFile::new(default_file);
        return match serve_file.oneshot(req).await {
            Ok(res) => Ok(res.into_response()),
            Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
        };
    }

    // 3. Default embedded static assets
    match static_path {
        "css/app.css" => {
            let css = include_str!("../../static/css/app.css");
            Ok((
                [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
                css,
            ).into_response())
        }
        "js/app.js" => {
            let js = include_str!("../../static/js/app.js");
            Ok((
                [(header::CONTENT_TYPE, "application/javascript; charset=utf-8")],
                js,
            ).into_response())
        }
        _ => Err(StatusCode::NOT_FOUND),
    }
}

// -------------------------------------------------------------
// Helper to render HTTP status error templates (e.g. 404.html, 403.html)
// -------------------------------------------------------------
pub fn render_status_response(
    state: &AppState,
    status: StatusCode,
    path: &str,
    message: &str,
    is_spa: bool,
) -> Response {
    let template_name = format!("{}.html", status.as_u16());
    let status_title = status.canonical_reason().unwrap_or("Error").to_string();
    let page_title = format!("{} - {}", status.as_u16(), status_title);

    let (nav_html, breadcrumbs_html, _) = if let Ok(conn) = state.db_pool.get() {
        build_page_chrome(&conn, &state.template_engine, path, &page_title)
    } else {
        (String::new(), String::new(), String::new())
    };

    let subtitle = match status {
        StatusCode::NOT_FOUND => "The requested page could not be found.",
        StatusCode::FORBIDDEN => "Access to this resource is restricted.",
        StatusCode::BAD_REQUEST => "The request could not be understood or was invalid.",
        _ => "An error occurred while processing your request.",
    };

    let ctx = json!({
        "status_code": status.as_u16(),
        "status_title": status_title,
        "title": page_title,
        "subtitle": subtitle,
        "message": message,
        "path": path,
        "navigation_html": nav_html,
        "breadcrumbs_html": breadcrumbs_html,
        "content_html": "",
        "toc_html": "",
        "config": state.config.to_json_value(),
    });

    if is_spa {
        let content_html = state
            .template_engine
            .render("components/error_content.html", &ctx)
            .unwrap_or_else(|_| format!("<h1>{}</h1><p>{}</p>", status, message));

        // Return JSON regions for SPA requests so the client doesn't break
        let resp_json = json!({
            "title": page_title,
            "url": path,
            "frontmatter": {},
            "config": state.config.to_json_value(),
            "regions": {
                "content": content_html,
                "toc": "",
                "breadcrumbs": breadcrumbs_html,
                "navigation": nav_html,
            }
        });
        (status, Json(resp_json)).into_response()
    } else {
        // Try rendering specific template, fallback to 404 or generic page
        let rendered = state
            .template_engine
            .render(&template_name, &ctx)
            .or_else(|_| state.template_engine.render("404.html", &ctx))
            .unwrap_or_else(|_| format!("<h1>{}</h1><p>{}</p>", status, message));

        (status, Html(rendered)).into_response()
    }
}

// -------------------------------------------------------------
// Auth Handlers: Login & Logout
// -------------------------------------------------------------
#[derive(Deserialize)]
struct LoginForm {
    username: String,
    password: String,
    redirect: Option<String>,
}

#[derive(Deserialize)]
struct LoginParams {
    redirect: Option<String>,
    error: Option<String>,
}

async fn login_page(
    State(state): State<AppState>,
    Query(params): Query<LoginParams>,
) -> Result<Html<String>, StatusCode> {
    let rendered = state
        .template_engine
        .render(
            "login.html",
            json!({
                "redirect_url": params.redirect.unwrap_or_else(|| "/".to_string()),
                "error": params.error,
                "config": state.config.to_json_value(),
            }),
        )
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Html(rendered))
}

async fn login_action(
    State(state): State<AppState>,
    axum::extract::Form(form): axum::extract::Form<LoginForm>,
) -> Result<Response, StatusCode> {
    let conn = state
        .db_pool
        .get()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let user_info: Option<(String, String, i32)> = conn
        .query_row(
            "SELECT password_hash, role, token_version FROM users WHERE username = ?1;",
            params![form.username],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .ok();

    if let Some((hash, role, ver)) = user_info {
        if AuthManager::verify_password(&form.password, &hash) {
            // Fetch ACL rules for user
            let mut rules = Vec::new();
            if let Ok(mut stmt) = conn.prepare(
                "SELECT verb, path_pattern FROM acls WHERE username = ?1 OR username = '*' ORDER BY priority ASC, id ASC;",
            ) {
                let rows = stmt.query_map(params![form.username], |r| {
                    Ok(AclRule {
                        v: r.get(0)?,
                        p: r.get(1)?,
                    })
                });
                if let Ok(r_iter) = rows {
                    for r in r_iter.flatten() {
                        rules.push(r);
                    }
                }
            }

            let token = state
                .auth_manager
                .create_jwt(&form.username, &role, ver, rules)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

            let target = form.redirect.filter(|s| !s.is_empty()).unwrap_or_else(|| "/".to_string());
            let mut response = Redirect::to(&target).into_response();
            let cookie = format!("jwt={}; Path=/; HttpOnly; SameSite=Lax", token);
            response.headers_mut().insert(
                header::SET_COOKIE,
                header::HeaderValue::from_str(&cookie).unwrap(),
            );
            return Ok(response);
        }
    }

    let target = format!(
        "/_user/login?error=Invalid%20credentials&redirect={}",
        urlencoding::encode(&form.redirect.unwrap_or_default())
    );
    Ok(Redirect::to(&target).into_response())
}

async fn logout_action() -> Response {
    let mut response = Redirect::to("/_user/login").into_response();
    let cookie = "jwt=; Path=/; Expires=Thu, 01 Jan 1970 00:00:00 GMT; HttpOnly; SameSite=Lax";
    response.headers_mut().insert(
        header::SET_COOKIE,
        header::HeaderValue::from_str(cookie).unwrap(),
    );
    response
}

// -------------------------------------------------------------
// Admin Handlers
// -------------------------------------------------------------
async fn admin_page(
    State(state): State<AppState>,
    req: Request,
) -> Result<Html<String>, StatusCode> {
    let claims = extract_claims_from_request(&req, &state.auth_manager);
    let username = claims.map(|c| c.username).unwrap_or_else(|| "admin".to_string());

    let conn = state
        .db_pool
        .get()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let mut users = Vec::new();
    let mut u_stmt = conn
        .prepare("SELECT username, role, token_version FROM users ORDER BY username ASC;")
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let u_rows = u_stmt
        .query_map([], |r| {
            Ok(json!({
                "username": r.get::<_, String>(0)?,
                "role": r.get::<_, String>(1)?,
                "token_version": r.get::<_, i32>(2)?,
            }))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    for u in u_rows.flatten() {
        users.push(u);
    }

    let mut acls = Vec::new();
    let mut a_stmt = conn
        .prepare("SELECT id, username, verb, path_pattern, priority FROM acls ORDER BY priority ASC, id ASC;")
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let a_rows = a_stmt
        .query_map([], |r| {
            Ok(json!({
                "id": r.get::<_, i64>(0)?,
                "username": r.get::<_, String>(1)?,
                "verb": r.get::<_, String>(2)?,
                "path_pattern": r.get::<_, String>(3)?,
                "priority": r.get::<_, i32>(4)?,
            }))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    for a in a_rows.flatten() {
        acls.push(a);
    }

    let rendered = state
        .template_engine
        .render(
            "admin.html",
            json!({
                "current_user": username,
                "users": users,
                "acls": acls,
                "config": state.config.to_json_value(),
            }),
        )
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Html(rendered))
}

#[derive(Deserialize)]
struct AdminCreateUserForm {
    username: String,
    password: String,
    role: String,
}

async fn admin_create_user(
    State(state): State<AppState>,
    axum::extract::Form(form): axum::extract::Form<AdminCreateUserForm>,
) -> Result<Redirect, StatusCode> {
    let conn = state
        .db_pool
        .get()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let hash = AuthManager::hash_password(&form.password)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    let _ = conn.execute(
        "INSERT INTO users (username, password_hash, role, token_version, created_at)
         VALUES (?1, ?2, ?3, 1, ?4)
         ON CONFLICT(username) DO UPDATE SET password_hash = excluded.password_hash, role = excluded.role;",
        params![form.username, hash, form.role, now],
    );

    Ok(Redirect::to("/_admin/"))
}

#[derive(Deserialize)]
struct UserForm {
    username: String,
}

async fn admin_delete_user(
    State(state): State<AppState>,
    axum::extract::Form(form): axum::extract::Form<UserForm>,
) -> Result<Redirect, StatusCode> {
    let conn = state
        .db_pool
        .get()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let _ = conn.execute("DELETE FROM users WHERE username = ?1;", params![form.username]);
    Ok(Redirect::to("/_admin/"))
}

async fn admin_bump_version(
    State(state): State<AppState>,
    axum::extract::Form(form): axum::extract::Form<UserForm>,
) -> Result<Redirect, StatusCode> {
    let conn = state
        .db_pool
        .get()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let _ = conn.execute(
        "UPDATE users SET token_version = token_version + 1 WHERE username = ?1;",
        params![form.username],
    );
    Ok(Redirect::to("/_admin/"))
}

#[derive(Deserialize)]
struct AdminCreateAclForm {
    username: String,
    verb: String,
    path_pattern: String,
    priority: i32,
}

async fn admin_create_acl(
    State(state): State<AppState>,
    axum::extract::Form(form): axum::extract::Form<AdminCreateAclForm>,
) -> Result<Redirect, StatusCode> {
    let conn = state
        .db_pool
        .get()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let _ = conn.execute(
        "INSERT INTO acls (username, verb, path_pattern, priority) VALUES (?1, ?2, ?3, ?4);",
        params![form.username, form.verb, form.path_pattern, form.priority],
    );
    Ok(Redirect::to("/_admin/"))
}

#[derive(Deserialize)]
struct AdminDeleteAclForm {
    id: i64,
}

async fn admin_delete_acl(
    State(state): State<AppState>,
    axum::extract::Form(form): axum::extract::Form<AdminDeleteAclForm>,
) -> Result<Redirect, StatusCode> {
    let conn = state
        .db_pool
        .get()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let _ = conn.execute("DELETE FROM acls WHERE id = ?1;", params![form.id]);
    Ok(Redirect::to("/_admin/"))
}

// -------------------------------------------------------------
// Helper routines
// -------------------------------------------------------------
fn normalize_url_path(path: &str) -> String {
    crate::crawler::normalize_slug_path(path)
}

fn extract_tags_from_fm(fm: &serde_json::Value) -> Vec<String> {
    if let Some(tags_val) = fm.get("tags") {
        if let Some(arr) = tags_val.as_array() {
            return arr
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect();
        } else if let Some(s) = tags_val.as_str() {
            return vec![s.to_string()];
        }
    }
    Vec::new()
}

fn get_all_distinct_tags(conn: &rusqlite::Connection) -> Vec<String> {
    let mut tags = std::collections::BTreeSet::new();
    if let Ok(mut stmt) = conn.prepare("SELECT frontmatter_json FROM pages WHERE frontmatter_json LIKE '%tags%';") {
        if let Ok(rows) = stmt.query_map([], |row| row.get::<_, String>(0)) {
            for json_str in rows.flatten() {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&json_str) {
                    for t in extract_tags_from_fm(&v) {
                        tags.insert(t);
                    }
                }
            }
        }
    }
    tags.into_iter().collect()
}

fn get_page_siblings(
    conn: &rusqlite::Connection,
    parent_path: &str,
    current_url: &str,
) -> (Option<String>, Option<String>) {
    let mut pages = Vec::new();
    if let Ok(mut stmt) = conn.prepare("SELECT url_path FROM pages WHERE parent_path = ?1 ORDER BY title ASC;") {
        if let Ok(rows) = stmt.query_map(params![parent_path], |row| row.get::<_, String>(0)) {
            for p in rows.flatten() {
                pages.push(p);
            }
        }
    }

    let mut prev = None;
    let mut next = None;

    if let Some(idx) = pages.iter().position(|p| p == current_url) {
        if idx > 0 {
            prev = Some(pages[idx - 1].clone());
        }
        if idx + 1 < pages.len() {
            next = Some(pages[idx + 1].clone());
        }
    }

    (prev, next)
}


async fn ensure_file_indexed(state: &AppState, url_path: &str) {
    if state.initialized.load(Ordering::Acquire) {
        return;
    }

    // Try finding on-demand file match
    let conn = match state.db_pool.get() {
        Ok(c) => c,
        Err(_) => return,
    };

    let exists: bool = conn
        .query_row(
            "SELECT 1 FROM pages WHERE url_path = ?1;",
            params![url_path],
            |_| Ok(true),
        )
        .unwrap_or(false);

    if !exists {
        // Fallback: check candidate files directly on disk
        let candidates = vec![
            format!("{}.md", url_path.trim_start_matches('/')),
            format!("{}/_index.md", url_path.trim_start_matches('/')),
            format!("{}/index.md", url_path.trim_start_matches('/')),
            format!("{}/readme.md", url_path.trim_start_matches('/')),
        ];

        for c in candidates {
            let disk_path = std::path::Path::new("content").join(&c);
            if disk_path.exists() {
                let _ = state.crawler.sync_single_file(&disk_path).await;
                break;
            }
        }
    }
}

#[derive(Debug, Clone)]
struct PageNavInfo {
    url_path: String,
    title: String,
    is_dir_index: bool,
}

#[derive(Debug)]
struct DirNavNode {
    path: String,
    name: String,
    has_index: bool,
    index_title: Option<String>,
    subdirs: std::collections::BTreeMap<String, DirNavNode>,
    pages: Vec<PageNavInfo>,
}

impl DirNavNode {
    fn new(path: String, name: String) -> Self {
        Self {
            path,
            name,
            has_index: false,
            index_title: None,
            subdirs: std::collections::BTreeMap::new(),
            pages: Vec::new(),
        }
    }
}

fn build_nav_tree(pages: Vec<PageNavInfo>, dir_titles: std::collections::HashMap<String, String>) -> DirNavNode {
    let mut root = DirNavNode::new("/".to_string(), "Home".to_string());
    let mut dir_indices: std::collections::HashMap<String, PageNavInfo> = std::collections::HashMap::new();
    let mut regular_pages: Vec<PageNavInfo> = Vec::new();

    for p in pages {
        if p.is_dir_index {
            dir_indices.insert(p.url_path.clone(), p);
        } else {
            regular_pages.push(p);
        }
    }

    for p in regular_pages {
        let parts: Vec<&str> = p.url_path.trim_matches('/').split('/').collect();
        if parts.is_empty() || (parts.len() == 1 && parts[0].is_empty()) {
            root.pages.push(p);
            continue;
        }

        let mut curr_node = &mut root;
        let mut curr_path = String::new();

        for (i, seg) in parts.iter().enumerate() {
            if i == parts.len() - 1 {
                curr_node.pages.push(p.clone());
            } else {
                curr_path.push('/');
                curr_path.push_str(seg);
                let seg_name = dir_titles.get(&curr_path).cloned().unwrap_or_else(|| seg.replace('-', " "));
                curr_node = curr_node.subdirs.entry(curr_path.clone()).or_insert_with(|| {
                    DirNavNode::new(curr_path.clone(), seg_name)
                });
            }
        }
    }

    fn apply_indices(node: &mut DirNavNode, indices: &std::collections::HashMap<String, PageNavInfo>) {
        if let Some(idx_page) = indices.get(&node.path) {
            node.has_index = true;
            node.index_title = Some(idx_page.title.clone());
        }
        for sub in node.subdirs.values_mut() {
            apply_indices(sub, indices);
        }
    }

    apply_indices(&mut root, &dir_indices);
    root
}

fn is_ancestor_or_self(ancestor_path: &str, target_url: &str) -> bool {
    let anc = ancestor_path.trim_end_matches('/');
    let target = target_url.trim_end_matches('/');
    if anc.is_empty() {
        return true;
    }
    if target == anc {
        return true;
    }
    target.starts_with(&format!("{}/", anc))
}

#[derive(Debug, Clone, Serialize)]
struct NavPageTemplateItem {
    url_path: String,
    title: String,
    is_active: bool,
}

#[derive(Debug, Clone, Serialize)]
struct NavSubdirTemplateItem {
    path: String,
    display_name: String,
    has_index: bool,
    is_open: bool,
    is_active: bool,
    subdirs: Vec<NavSubdirTemplateItem>,
    pages: Vec<NavPageTemplateItem>,
}

#[derive(Debug, Clone, Serialize)]
struct NavTreeTemplateContext {
    subdirs: Vec<NavSubdirTemplateItem>,
    pages: Vec<NavPageTemplateItem>,
}

impl DirNavNode {
    fn to_template_context(&self, active_url: &str) -> NavTreeTemplateContext {
        let mut subdirs = Vec::new();
        for (path, sub) in &self.subdirs {
            let display_name = sub.index_title.as_deref().unwrap_or(&sub.name).to_string();
            let is_active = active_url == path;
            let is_open = is_ancestor_or_self(path, active_url);
            let child_ctx = sub.to_template_context(active_url);

            subdirs.push(NavSubdirTemplateItem {
                path: path.clone(),
                display_name,
                has_index: sub.has_index,
                is_open,
                is_active,
                subdirs: child_ctx.subdirs,
                pages: child_ctx.pages,
            });
        }

        let mut pages = Vec::new();
        for page in &self.pages {
            let is_active = page.url_path == active_url;
            pages.push(NavPageTemplateItem {
                url_path: page.url_path.clone(),
                title: page.title.clone(),
                is_active,
            });
        }

        NavTreeTemplateContext { subdirs, pages }
    }
}

fn build_page_chrome(
    conn: &rusqlite::Connection,
    template_engine: &TemplateEngine,
    url_path: &str,
    page_title: &str,
) -> (String, String, String) {
    // 1. Hierarchical Navigation Tree
    let mut pages = Vec::new();
    if let Ok(mut stmt) = conn.prepare("SELECT url_path, title, is_dir_index FROM pages ORDER BY depth ASC, title ASC;") {
        let rows = stmt.query_map([], |r| {
            Ok(PageNavInfo {
                url_path: r.get(0)?,
                title: r.get(1)?,
                is_dir_index: r.get::<_, i32>(2)? == 1,
            })
        });
        if let Ok(mapped) = rows {
            for p in mapped.flatten() {
                pages.push(p);
            }
        }
    }

    let mut dir_titles = std::collections::HashMap::new();
    if let Ok(mut stmt) = conn.prepare("SELECT url_path, title FROM directories WHERE title IS NOT NULL;") {
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)));
        if let Ok(mapped) = rows {
            for (u, t) in mapped.flatten() {
                dir_titles.insert(u, t);
            }
        }
    }

    let root_node = build_nav_tree(pages, dir_titles);
    let nav_tree_ctx = root_node.to_template_context(url_path);
    let nav_html = template_engine
        .render("components/nav_tree.html", json!({ "tree": nav_tree_ctx }))
        .unwrap_or_default();

    // 2. Breadcrumbs
    #[derive(Serialize)]
    struct BreadcrumbItem {
        url: String,
        title: String,
        is_current: bool,
    }

    let mut crumbs = vec![BreadcrumbItem {
        url: "/".to_string(),
        title: "Home".to_string(),
        is_current: url_path == "/",
    }];

    if url_path != "/" {
        let parts: Vec<&str> = url_path.trim_matches('/').split('/').collect();
        let mut curr = String::new();
        for (i, p) in parts.iter().enumerate() {
            curr.push('/');
            curr.push_str(p);
            let is_last = i == parts.len() - 1;
            let title = if is_last {
                page_title.to_string()
            } else {
                p.replace('-', " ")
            };
            crumbs.push(BreadcrumbItem {
                url: curr.clone(),
                title,
                is_current: is_last,
            });
        }
    }

    let breadcrumbs_html = template_engine
        .render("components/breadcrumbs.html", json!({ "crumbs": crumbs }))
        .unwrap_or_default();

    // 3. TOC placeholder (generated during markdown parsing or queried)
    let toc_html = String::new();

    (nav_html, breadcrumbs_html, toc_html)
}
