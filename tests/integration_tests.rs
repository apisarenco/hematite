use hematite::auth::{AclEngine, AclRule, AuthManager};
use hematite::crawler::derive_path_metadata;
use hematite::db::resolve_target;
use hematite::markdown::MarkdownEngine;
use hematite::plugin::PluginRegistry;
use std::sync::Arc;

#[test]
fn test_obsidian_image_embeds() {
    let mut reg = PluginRegistry::new();
    hematite::plugins::register_all(&mut reg);
    let engine = MarkdownEngine::new(Arc::new(reg));

    let md = r#"
# Muk Character Sheet

![[Muk.jpg|right lp|200]]

Session memory:
![[session 001.jpg]]

Item:
![[Ring of Winter.jpg|400]]

Dimensions:
![[map.png|800x600]]
"#;

    let media_resolver = |target: &str| -> Option<String> {
        match target {
            "Muk.jpg" => Some("/_media/04_images/01_characters/pc/Muk.jpg".to_string()),
            "session 001.jpg" => Some("/_media/04_images/00_sessions/session%20001.jpg".to_string()),
            _ => None,
        }
    };

    let parsed = engine.parse_with_context(
        md,
        "Muk",
        Some("/03-player-knowledge/muk"),
        None,
        Some(&media_resolver),
    );

    // Verify Muk.jpg
    assert!(parsed.html.contains("<img src=\"/_media/04_images/01_characters/pc/Muk.jpg\""));
    assert!(parsed.html.contains("width=\"200\""));
    assert!(parsed.html.contains("class=\"align-right lp\""));

    // Verify session 001.jpg
    assert!(parsed.html.contains("<img src=\"/_media/04_images/00_sessions/session%20001.jpg\""));
    assert!(parsed.html.contains("alt=\"session 001.jpg\""));

    // Verify Ring of Winter fallback URL encoding
    assert!(parsed.html.contains("<img src=\"/_media/Ring%20of%20Winter.jpg\""));
    assert!(parsed.html.contains("width=\"400\""));

    // Verify map.png dimensions WxH
    assert!(parsed.html.contains("width=\"800\""));
    assert!(parsed.html.contains("height=\"600\""));
}

#[test]
fn test_wikilinks_and_markdown() {
    let mut reg = PluginRegistry::new();
    hematite::plugins::register_all(&mut reg);
    let engine = MarkdownEngine::new(Arc::new(reg));

    let md = r#"---
title: System Architecture
tags: [core, arch]
---

# Overview

See [[Networking]] and [[Security|Secure Tunnel]] and [[Fighter#Extra Attack (Level 5)|Extra Attack]].
"#;

    let catalog = vec![
        ("/docs/system/networking".to_string(), "docs/system/networking.md".to_string(), "Networking Setup".to_string()),
        ("/security/secure-tunnel".to_string(), "security/Security.md".to_string(), "Security System".to_string()),
        ("/classes/fighter".to_string(), "06_rules/Classes/Fighter.md".to_string(), "Fighter Class".to_string()),
    ];

    let resolver = |source_url: &str, target_raw: &str| -> (String, Option<String>) {
        if let Some((url, title)) = resolve_target(source_url, target_raw, &catalog) {
            (url, Some(title))
        } else {
            (format!("/{}", target_raw), None)
        }
    };

    let parsed = engine.parse_with_context(md, "Default Title", Some("/overview"), Some(&resolver), None);
    assert_eq!(parsed.title, "System Architecture");
    assert!(parsed.links.contains(&"Networking".to_string()));
    assert!(parsed.links.contains(&"Security".to_string()));
    assert!(parsed.links.contains(&"Fighter".to_string()));
    assert!(parsed.html.contains("/docs/system/networking"));
    assert!(parsed.html.contains("/security/secure-tunnel"));
    assert!(parsed.html.contains("Networking Setup")); // uses resolved page title when no display text
    assert!(parsed.html.contains("Secure Tunnel")); // uses explicit display text pipe
    assert!(parsed.html.contains(r#"href="/classes/fighter#extra-attack-level-5""#));
    assert!(parsed.html.contains(">Extra Attack</a>"));
}

#[test]
fn test_path_metadata_derivation() {
    let (url, parent, depth, is_dir_index, title) = derive_path_metadata("docs/system/networking.md");
    assert_eq!(url, "/docs/system/networking");
    assert_eq!(parent, "/docs/system");
    assert_eq!(depth, 3);
    assert!(!is_dir_index);
    assert_eq!(title, "Networking");

    let (url, parent, depth, is_dir_index, title) = derive_path_metadata("02_party knowledge/session 01.md");
    assert_eq!(url, "/02-party-knowledge/session-01");
    assert_eq!(parent, "/02-party-knowledge");
    assert_eq!(depth, 2);
    assert!(!is_dir_index);
    assert_eq!(title, "Session 01");

    let (url, parent, depth, is_dir_index, title) = derive_path_metadata("docs/system/index.md");
    assert_eq!(url, "/docs/system");
    assert_eq!(parent, "/docs");
    assert_eq!(depth, 2);
    assert!(is_dir_index);
    assert_eq!(title, "System");

    let (url, parent, depth, is_dir_index, title) = derive_path_metadata("docs/system/_index.md");
    assert_eq!(url, "/docs/system");
    assert_eq!(parent, "/docs");
    assert_eq!(depth, 2);
    assert!(is_dir_index);
    assert_eq!(title, "System");

    let (url, parent, depth, is_dir_index, title) = derive_path_metadata("index.md");
    assert_eq!(url, "/");
    assert_eq!(parent, "");
    assert_eq!(depth, 0);
    assert!(is_dir_index);
    assert_eq!(title, "Home");
}

#[test]
fn test_acl_engine() {
    let rules = vec![
        AclRule {
            v: "ALLOW".to_string(),
            p: "/docs/**".to_string(),
        },
        AclRule {
            v: "DENY".to_string(),
            p: "/docs/secret/**".to_string(),
        },
    ];

    assert!(AclEngine::check_permission("/docs/guide", &rules));
    assert!(AclEngine::check_permission("/docs/system/network", &rules));
    assert!(!AclEngine::check_permission("/docs/secret/passwords", &rules));
    assert!(!AclEngine::check_permission("/other", &rules)); // default deny
}

#[test]
fn test_jwt_auth_and_versioning() {
    let auth = AuthManager::new("secret-key-12345678901234567890");
    let pw = "secretpass";
    let hash = AuthManager::hash_password(pw).unwrap();
    assert!(AuthManager::verify_password(pw, &hash));
    assert!(!AuthManager::verify_password("wrong", &hash));

    let rules = vec![AclRule {
        v: "ALLOW".to_string(),
        p: "/**".to_string(),
    }];

    let token = auth.create_jwt("admin", "admin", 1, rules).unwrap();
    let claims = auth.verify_jwt(&token).unwrap();
    assert_eq!(claims.username, "admin");
    assert_eq!(claims.ver, 1);
    assert_eq!(claims.role, "admin");
}

#[test]
fn test_wikilink_resolver() {
    let pages = vec![
        ("/system/arch".to_string(), "system/arch.md".to_string(), "Architecture".to_string()),
        ("/notes/arch".to_string(), "notes/arch.md".to_string(), "Notes Arch".to_string()),
        ("/docs/intro".to_string(), "docs/intro.md".to_string(), "Introduction".to_string()),
    ];

    // Sibling relative resolution
    let resolved = resolve_target("/system/overview", "arch", &pages);
    assert_eq!(resolved, Some(("/system/arch".to_string(), "Architecture".to_string())));

    // Slug match
    let resolved_slug = resolve_target("/other", "intro", &pages);
    assert_eq!(resolved_slug, Some(("/docs/intro".to_string(), "Introduction".to_string())));

    // Filename-based resolution with .md extension (e.g. [[Session 14-s.md]])
    let session_pages = vec![
        ("/02-party-knowledge/01-sessions/Session-14-s".to_string(), "02_party knowledge/01_sessions/Session 14-s.md".to_string(), "Session 14".to_string()),
    ];
    let resolved_file_ext = resolve_target("/02-party-knowledge/01-sessions/Session-Log-s", "Session 14-s.md", &session_pages);
    assert_eq!(resolved_file_ext, Some(("/02-party-knowledge/01-sessions/Session-14-s".to_string(), "Session 14".to_string())));

    let resolved_file_no_ext = resolve_target("/02-party-knowledge/01-sessions/Session-Log-s", "Session 14-s", &session_pages);
    assert_eq!(resolved_file_no_ext, Some(("/02-party-knowledge/01-sessions/Session-14-s".to_string(), "Session 14".to_string())));

    // Wikilink with anchor / heading [[Fighter#Extra Attack (Level 5)]]
    let fighter_pages = vec![
        ("/classes/fighter".to_string(), "06_rules/Classes/Fighter.md".to_string(), "Fighter".to_string()),
    ];
    let resolved_anchor = resolve_target("/characters/muk", "Fighter#Extra Attack (Level 5)", &fighter_pages);
    assert_eq!(resolved_anchor, Some(("/classes/fighter#extra-attack-level-5".to_string(), "Fighter".to_string())));
}

#[test]
fn test_obsidian_admonitions() {
    let mut reg = PluginRegistry::new();
    hematite::plugins::register_all(&mut reg);
    let engine = MarkdownEngine::new(Arc::new(reg));

    let md = r#"
> [!note] Simple note title
> This is a standard note body.

> [!example]- Stats, Abilities, Skills
> ##### **Stats**
> | HP | AC |
> | --- | --- |
> | 43 | 17 |

> [!danger]+ Critical Warning
> Don't do this!
"#;

    let parsed = engine.parse(md, "Admonitions Test");
    println!("RENDERED HTML:\n{}", parsed.html);

    // Standard static note callout
    assert!(parsed.html.contains(r#"<div class="admonition admonition-note"><div class="admonition-title">"#));
    assert!(parsed.html.contains(r#"<span class="admonition-title-text">Simple note title</span>"#));
    assert!(parsed.html.contains("<p>This is a standard note body.</p>"));
    assert!(parsed.html.contains("</div></div>"));

    // Collapsible collapsed callout: <details class="admonition admonition-example">
    assert!(parsed.html.contains(r#"<details class="admonition admonition-example"><summary class="admonition-title">"#));
    assert!(parsed.html.contains(r#"<span class="admonition-title-text">Stats, Abilities, Skills</span>"#));
    assert!(parsed.html.contains(r#"<h5><strong>Stats</strong></h5>"#));
    assert!(parsed.html.contains("<table>"));
    assert!(parsed.html.contains("</div></details>"));

    // Collapsible open callout: <details class="admonition admonition-danger" open>
    assert!(parsed.html.contains(r#"<details class="admonition admonition-danger" open><summary class="admonition-title">"#));
    assert!(parsed.html.contains(r#"<span class="admonition-title-text">Critical Warning</span>"#));
    assert!(parsed.html.contains("<p>Don't do this!</p>"));
}

#[tokio::test]
async fn test_extensibility_hooks_lua_and_rhai() {
    use std::fs;
    let temp_dir = std::env::temp_dir().join(format!("hematite_test_hooks_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    let content_dir = temp_dir.join("content");
    let scripts_dir = temp_dir.join("scripts");
    let file_hooks_dir = scripts_dir.join("hooks/file");
    let dir_hooks_dir = scripts_dir.join("hooks/directory");
    let db_path = temp_dir.join("test.db");

    fs::create_dir_all(&content_dir).unwrap();
    fs::create_dir_all(&file_hooks_dir).unwrap();
    fs::create_dir_all(&dir_hooks_dir).unwrap();

    // Create a subfolder with sessions
    let sessions_dir = content_dir.join("sessions");
    fs::create_dir_all(&sessions_dir).unwrap();
    fs::write(sessions_dir.join("session-01.md"), "# Session 1 notes\nSome content here.").unwrap();
    fs::write(sessions_dir.join("_index.md"), "---\ntitle: Campaign Sessions\n---\nOverview of all campaign sessions.").unwrap();

    // 1. File hook in Lua: prefix session titles with "[Session] "
    let lua_file_hook = r#"
if string.find(entity.url_path, "/sessions/session") then
    entity.title = "[Session] " .. entity.title
end
return entity
"#;
    fs::write(file_hooks_dir.join("01_session_renamer.lua"), lua_file_hook).unwrap();

    // 2. File hook in Rhai: chain after Lua, append " (Verified)" to all page titles
    let rhai_file_hook = r#"
entity.title = entity.title + " (Verified)";
return entity;
"#;
    fs::write(file_hooks_dir.join("02_verifier.rhai"), rhai_file_hook).unwrap();

    // 3. Directory hook in Lua: rename directory if it contains files
    let lua_dir_hook = r#"
if entity.url_path == "/sessions" then
    entity.title = "Renamed Sessions (" .. #entity.files .. " files)"
end
return entity
"#;
    fs::write(dir_hooks_dir.join("01_dir_formatter.lua"), lua_dir_hook).unwrap();

    // Run crawler
    let mut reg = PluginRegistry::new();
    hematite::plugins::register_all(&mut reg);
    let engine = MarkdownEngine::new(std::sync::Arc::new(reg));

    let (writer, _handle) = hematite::db::DbActor::spawn(&db_path, &content_dir, 64).unwrap();
    let script_engine = hematite::scripts::ScriptEngine::new(&scripts_dir);
    let config = hematite::config::HematiteConfig::default();
    let crawler = hematite::crawler::Crawler::new(content_dir.clone(), engine, writer.clone(), script_engine, config);

    crawler.initial_sync(std::collections::HashMap::new()).await.unwrap();

    // Verify page title was transformed in sequence by Lua then Rhai
    let pages = writer.get_all_pages_for_hooks().await.unwrap();
    let session_page = pages.iter().find(|p| p.url_path == "/sessions/session-01").expect("session page exists");
    assert_eq!(session_page.title, "[Session] Session 1 notes (Verified)");

    let index_page = pages.iter().find(|p| p.url_path == "/sessions").expect("index page exists");
    assert_eq!(index_page.title, "Campaign Sessions (Verified)");

    // Verify directory hook executed
    let dirs = writer.get_all_directories_for_hooks().await.unwrap();
    let session_dir = dirs.iter().find(|d| d.url_path == "/sessions").expect("sessions dir exists");
    assert_eq!(session_dir.title, "Renamed Sessions (1 files)");

    // Clean up
    let _ = fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_hematite_yaml_config_system() {
    use std::fs;
    use tower::ServiceExt;

    let temp_dir = std::env::temp_dir().join(format!("hematite_test_cfg_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    let content_dir = temp_dir.join("content");
    let scripts_dir = temp_dir.join("scripts");
    let file_hooks_dir = scripts_dir.join("hooks/file");
    let db_path = temp_dir.join("test.db");
    fs::create_dir_all(&content_dir).unwrap();
    fs::create_dir_all(&file_hooks_dir).unwrap();

    // 1. Write hematite.yaml with website name, base_url, theme, and free-form settings
    let yaml_content = r#"
name: "My Custom RPG Wiki"
base_url: "https://wiki.example.com"
theme: "dark-fantasy"
author: "Dungeon Master"
custom_feature_flag: true
dnd_edition: "5e"
"#;
    let config_path = temp_dir.join("hematite.yaml");
    fs::write(&config_path, yaml_content).unwrap();

    // Load config
    let config = hematite::config::HematiteConfig::load_or_default(Some(&config_path), Some(&content_dir));
    assert_eq!(config.name, "My Custom RPG Wiki");
    assert_eq!(config.base_url, "https://wiki.example.com");
    assert_eq!(config.theme, "dark-fantasy");
    assert_eq!(config.extra.get("author").and_then(|v| v.as_str()), Some("Dungeon Master"));
    assert_eq!(config.extra.get("custom_feature_flag").and_then(|v| v.as_bool()), Some(true));

    // 2. Write test markdown file
    fs::write(content_dir.join("index.md"), "# Welcome to the Wiki").unwrap();

    // 3. Write a Lua script that uses `config`
    let lua_hook = r#"
entity.title = config.name .. " - " .. entity.title .. " (Theme: " .. config.theme .. ", Edition: " .. tostring(config.dnd_edition) .. ")"
return entity
"#;
    fs::write(file_hooks_dir.join("01_config_title.lua"), lua_hook).unwrap();

    let mut reg = PluginRegistry::new();
    hematite::plugins::register_all(&mut reg);
    let engine = MarkdownEngine::new(std::sync::Arc::new(reg));

    let (writer, _handle) = hematite::db::DbActor::spawn(&db_path, &content_dir, 64).unwrap();
    let script_engine = hematite::scripts::ScriptEngine::new(&scripts_dir);
    let crawler = std::sync::Arc::new(hematite::crawler::Crawler::new(
        content_dir.clone(),
        engine,
        writer.clone(),
        script_engine,
        config.clone(),
    ));

    crawler.initial_sync(std::collections::HashMap::new()).await.unwrap();

    // Verify script transformed entity title using config
    let pages = writer.get_all_pages_for_hooks().await.unwrap();
    let page = pages.iter().find(|p| p.url_path == "/").expect("index page exists");
    assert_eq!(page.title, "My Custom RPG Wiki - Welcome to the Wiki (Theme: dark-fantasy, Edition: 5e)");

    // 4. Test theme template rendering with config context
    let manager = r2d2_sqlite::SqliteConnectionManager::file(&db_path);
    let pool = r2d2::Pool::builder().max_size(8).build(manager).unwrap();
    {
        let conn = pool.get().unwrap();
        conn.execute("INSERT OR IGNORE INTO users (username, password_hash, role, token_version, created_at) VALUES ('*', '', 'anonymous', 1, 0);", []).unwrap();
        conn.execute("INSERT INTO acls (username, verb, path_pattern, priority) VALUES ('*', 'ALLOW', '/**', 0);", []).unwrap();
    }

    let template_engine = std::sync::Arc::new(hematite::template::TemplateEngine::new().unwrap());
    let auth_manager = std::sync::Arc::new(AuthManager::new("secret-key-12345678901234567890"));
    let initialized = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));

    let state = hematite::server::AppState {
        db_pool: pool,
        auth_manager,
        template_engine,
        crawler,
        writer: writer.clone(),
        initialized,
        content_root: content_dir.clone(),
        config: std::sync::Arc::new(config),
        theme_dir: temp_dir.join("themes/default"),
        reconciler_notify: std::sync::Arc::new(tokio::sync::Notify::new()),
    };

    let app = hematite::server::create_router(state.clone());

    // SSR Request: verify <title> and data-theme contain config values
    let req = axum::http::Request::builder()
        .uri("/")
        .body(axum::body::Body::empty())
        .unwrap();

    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), axum::http::StatusCode::OK);

    let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let body_str = String::from_utf8_lossy(&body_bytes);
    assert!(body_str.contains("<title>My Custom RPG Wiki - Welcome to the Wiki (Theme: dark-fantasy, Edition: 5e) - My Custom RPG Wiki</title>"));
    assert!(body_str.contains(r#"data-theme="dark-fantasy""#));

    // SPA API Request: verify config is provided in json response
    let req_api = axum::http::Request::builder()
        .uri("/_api/v1/page?path=/")
        .body(axum::body::Body::empty())
        .unwrap();

    let resp_api = app.oneshot(req_api).await.unwrap();
    assert_eq!(resp_api.status(), axum::http::StatusCode::OK);
    let api_bytes = axum::body::to_bytes(resp_api.into_body(), usize::MAX).await.unwrap();
    let api_json: serde_json::Value = serde_json::from_slice(&api_bytes).unwrap();
    assert_eq!(api_json["config"]["name"], "My Custom RPG Wiki");
    assert_eq!(api_json["config"]["theme"], "dark-fantasy");
    assert_eq!(api_json["config"]["author"], "Dungeon Master");

    let _ = fs::remove_dir_all(&temp_dir);
}
#[tokio::test]
async fn test_status_404_and_403_templates() {
    use std::fs;
    use tower::ServiceExt;
    let temp_dir = std::env::temp_dir().join(format!("hematite_test_status_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    let content_dir = temp_dir.join("content");
    let db_path = temp_dir.join("test.db");
    fs::create_dir_all(&content_dir).unwrap();
    fs::write(content_dir.join("index.md"), "# Welcome\nHome page").unwrap();

    let mut reg = PluginRegistry::new();
    hematite::plugins::register_all(&mut reg);
    let engine = MarkdownEngine::new(std::sync::Arc::new(reg));

    let (writer, _handle) = hematite::db::DbActor::spawn(&db_path, &content_dir, 64).unwrap();
    let script_engine = hematite::scripts::ScriptEngine::new(&temp_dir.join("scripts"));
    let config = std::sync::Arc::new(hematite::config::HematiteConfig::default());
    let crawler = std::sync::Arc::new(hematite::crawler::Crawler::new(content_dir.clone(), engine, writer.clone(), script_engine, (*config).clone()));

    crawler.initial_sync(std::collections::HashMap::new()).await.unwrap();

    let manager = r2d2_sqlite::SqliteConnectionManager::file(&db_path);
    let pool = r2d2::Pool::builder().max_size(8).build(manager).unwrap();

    // Insert default public user and ACL rule so anonymous requests can access content
    {
        let conn = pool.get().unwrap();
        conn.execute("INSERT OR IGNORE INTO users (username, password_hash, role, token_version, created_at) VALUES ('*', '', 'anonymous', 1, 0);", []).unwrap();
        conn.execute("INSERT INTO acls (username, verb, path_pattern, priority) VALUES ('*', 'ALLOW', '/**', 0);", []).unwrap();
    }

    let template_engine = std::sync::Arc::new(hematite::template::TemplateEngine::new().unwrap());
    let auth_manager = std::sync::Arc::new(AuthManager::new("secret-key-12345678901234567890"));
    let initialized = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));

    let state = hematite::server::AppState {
        db_pool: pool,
        auth_manager,
        template_engine,
        crawler,
        writer: writer.clone(),
        initialized,
        content_root: content_dir.clone(),
        config,
        theme_dir: temp_dir.join("themes/default"),
        reconciler_notify: std::sync::Arc::new(tokio::sync::Notify::new()),
    };

    let app = hematite::server::create_router(state.clone());

    // 1. Test 404 Not Found response
    let req_404 = axum::http::Request::builder()
        .uri("/nonexistent-page")
        .body(axum::body::Body::empty())
        .unwrap();

    let resp_404 = app.clone().oneshot(req_404).await.unwrap();
    assert_eq!(resp_404.status(), axum::http::StatusCode::NOT_FOUND);

    let body_bytes = axum::body::to_bytes(resp_404.into_body(), usize::MAX).await.unwrap();
    let body_str = String::from_utf8_lossy(&body_bytes);
    assert!(body_str.contains("404 - Page Not Found"));
    assert!(body_str.contains("history.back()"));
    assert!(body_str.contains("/_static/js/app.js"));

    // 2. Test 403 Forbidden template
    let resp_403 = hematite::server::render_status_response(
        &state,
        axum::http::StatusCode::FORBIDDEN,
        "/secret/area",
        "You do not have permission to view this resource.",
        false,
    );
    assert_eq!(resp_403.status(), axum::http::StatusCode::FORBIDDEN);
    let body_403_bytes = axum::body::to_bytes(resp_403.into_body(), usize::MAX).await.unwrap();
    let body_403_str = String::from_utf8_lossy(&body_403_bytes);
    assert!(body_403_str.contains("403 - Forbidden"));
    assert!(body_403_str.contains("history.back()"));

    // 3. Test SPA API 404 response
    let req_spa_404 = axum::http::Request::builder()
        .uri("/_api/v1/page?path=/nonexistent-page")
        .body(axum::body::Body::empty())
        .unwrap();

    let resp_spa_404 = app.clone().oneshot(req_spa_404).await.unwrap();
    assert_eq!(resp_spa_404.status(), axum::http::StatusCode::NOT_FOUND);
    let spa_bytes = axum::body::to_bytes(resp_spa_404.into_body(), usize::MAX).await.unwrap();
    let spa_json: serde_json::Value = serde_json::from_slice(&spa_bytes).unwrap();
    assert!(spa_json.get("regions").is_some());
    let content = spa_json["regions"]["content"].as_str().unwrap();
    assert!(content.contains("404 - Page Not Found"));
    assert!(content.contains("history.back()"));

    let _ = fs::remove_dir_all(&temp_dir);
}
#[tokio::test]
async fn test_custom_theme_features_and_overrides() {
    use std::fs;
    use tower::ServiceExt;

    let temp_dir = std::env::temp_dir().join(format!("hematite_test_theme_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    let content_dir = temp_dir.join("content");
    let themes_dir = temp_dir.join("themes");
    let mock_theme_dir = themes_dir.join("bemm");
    let mock_templates_dir = mock_theme_dir.join("templates");
    let mock_static_img_dir = mock_theme_dir.join("static/img");
    let db_path = temp_dir.join("test.db");
    fs::create_dir_all(&content_dir).unwrap();
    fs::create_dir_all(&mock_templates_dir).unwrap();
    fs::create_dir_all(&mock_static_img_dir).unwrap();

    // Create self-contained mock theme files (templates + static assets)
    fs::write(
        mock_static_img_dir.join("staff.svg"),
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><circle cx="5" cy="5" r="4"/></svg>"#,
    )
    .unwrap();

    let mock_page_html = r#"<!DOCTYPE html>
<html>
<head><title>{{ title }} - {{ config.name }}</title></head>
<body data-theme="{{ config.theme }}">
    <header>{{ config.name }}</header>
    <div class="tags-section">
        <div class="tags-section-header">Arcane Tags</div>
        {% for t in tags %}<span>{{ t }}</span>{% endfor %}
    </div>
    <div class="user-panel">
        <span class="user-name">{{ user.username }}</span>
    </div>
    {% if not is_search_page and title %}
    <h1 class="page-title-header" id="page-primary-title">{{ title }}</h1>
    {% endif %}
    <article class="prose" id="region-content">{{ content_html | safe }}</article>
</body>
</html>"#;
    fs::write(mock_templates_dir.join("page.html"), mock_page_html).unwrap();

    // 1. Create markdown with frontmatter tags and H1
    let md = r#"---
tags:
  - Adventure
  - Quest
---
# The Golden Chalice

This is the content of the quest.
"#;
    fs::write(content_dir.join("chalice.md"), md).unwrap();

    let mut reg = PluginRegistry::new();
    hematite::plugins::register_all(&mut reg);
    let engine = MarkdownEngine::new(std::sync::Arc::new(reg));

    let (writer, _handle) = hematite::db::DbActor::spawn(&db_path, &content_dir, 64).unwrap();
    let script_engine = hematite::scripts::ScriptEngine::new(&temp_dir.join("scripts"));
    let mut config = hematite::config::HematiteConfig::default();
    config.theme = "bemm".to_string();
    config.name = "Bemm Realm".to_string();
    config.anonymous_user_display = "Mysterious Wanderer".to_string();

    let crawler = std::sync::Arc::new(hematite::crawler::Crawler::new(
        content_dir.clone(),
        engine,
        writer.clone(),
        script_engine,
        config.clone(),
    ));

    crawler.initial_sync(std::collections::HashMap::new()).await.unwrap();

    let manager = r2d2_sqlite::SqliteConnectionManager::file(&db_path);
    let pool = r2d2::Pool::builder().max_size(8).build(manager).unwrap();
    {
        let conn = pool.get().unwrap();
        conn.execute("INSERT OR IGNORE INTO users (username, password_hash, role, token_version, created_at) VALUES ('*', '', 'anonymous', 1, 0);", []).unwrap();
        conn.execute("INSERT INTO acls (username, verb, path_pattern, priority) VALUES ('*', 'ALLOW', '/**', 0);", []).unwrap();
    }

    let template_engine = std::sync::Arc::new(hematite::template::TemplateEngine::with_theme_override(Some(&mock_theme_dir)).unwrap());
    let auth_manager = std::sync::Arc::new(AuthManager::new("secret-key-12345678901234567890"));
    let initialized = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));

    let state = hematite::server::AppState {
        db_pool: pool,
        auth_manager,
        template_engine,
        crawler,
        writer: writer.clone(),
        initialized,
        content_root: content_dir.clone(),
        config: std::sync::Arc::new(config),
        theme_dir: mock_theme_dir.clone(),
        reconciler_notify: std::sync::Arc::new(tokio::sync::Notify::new()),
    };

    let app = hematite::server::create_router(state);

    // Test static asset serve from theme override (e.g. staff.svg)
    let req_static = axum::http::Request::builder()
        .uri("/_static/img/staff.svg")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp_static = app.clone().oneshot(req_static).await.unwrap();
    assert_eq!(resp_static.status(), axum::http::StatusCode::OK);

    // Test SSR render with mock theme
    let req_ssr = axum::http::Request::builder()
        .uri("/chalice")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp_ssr = app.clone().oneshot(req_ssr).await.unwrap();
    assert_eq!(resp_ssr.status(), axum::http::StatusCode::OK);
    let body_bytes = axum::body::to_bytes(resp_ssr.into_body(), usize::MAX).await.unwrap();
    let body_str = String::from_utf8_lossy(&body_bytes);

    // Check theme elements and anonymous_user_display config
    assert!(body_str.contains("Bemm Realm"));
    assert!(body_str.contains("Arcane Tags"));
    assert!(body_str.contains("Adventure"));
    assert!(body_str.contains("Mysterious Wanderer"));
    // Since title is taken from H1, verify that H1 is removed from content and rendered as the primary title
    assert!(body_str.contains(r#"id="page-primary-title">The Golden Chalice</h1>"#));
    assert!(body_str.contains("This is the content of the quest."));
    // Ensure the content itself doesn't contain a duplicate H1
    assert!(!body_str.contains(r#"<article class="prose" id="region-content"><h1>The Golden Chalice</h1>"#));

    // Test dedicated search page
    let req_search = axum::http::Request::builder()
        .uri("/_search?q=Chalice")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp_search = app.oneshot(req_search).await.unwrap();
    assert_eq!(resp_search.status(), axum::http::StatusCode::OK);
    let search_bytes = axum::body::to_bytes(resp_search.into_body(), usize::MAX).await.unwrap();
    let _search_str = String::from_utf8_lossy(&search_bytes);
    let _ = fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_ast_cbor_and_element_renderer_override() {
    use std::fs;
    use tower::ServiceExt;

    let temp_dir = std::env::temp_dir().join(format!(
        "hematite_test_ast_{}",
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    let content_dir = temp_dir.join("content");
    let theme_dir = temp_dir.join("themes/custom");
    let element_templates_dir = theme_dir.join("templates/elements");
    let db_path = temp_dir.join("test.db");

    fs::create_dir_all(&content_dir).unwrap();
    fs::create_dir_all(&element_templates_dir).unwrap();

    // 1. Create a custom element template override for admonitions
    let custom_admonition_html = r#"<aside class="custom-callout-override callout-{{ block.data.kind }}">
  <div class="custom-header">📢 {{ block.data.title or block.data.kind }}</div>
  <div class="custom-body">{{ rendered_html | safe }}</div>
</aside>"#;
    fs::write(element_templates_dir.join("admonition.html"), custom_admonition_html).unwrap();

    // 2. Write markdown with Obsidian callout
    let doc_content = r#"---
title: "Testing AST Elements"
---

# Testing AST Elements

> [!TIP] Pro Tip
> This is a test admonition block that should be rendered with the custom element template!
"#;
    fs::write(content_dir.join("tip.md"), doc_content).unwrap();

    let mut reg = PluginRegistry::new();
    hematite::plugins::register_all(&mut reg);
    let engine = MarkdownEngine::new(std::sync::Arc::new(reg));

    let (writer, _handle) = hematite::db::DbActor::spawn(&db_path, &content_dir, 64).unwrap();
    let script_engine = hematite::scripts::ScriptEngine::new(&temp_dir.join("scripts"));
    let mut config = hematite::config::HematiteConfig::default();
    config.theme = "custom".to_string();

    let crawler = std::sync::Arc::new(hematite::crawler::Crawler::new(
        content_dir.clone(),
        engine,
        writer.clone(),
        script_engine,
        config.clone(),
    ));

    crawler.initial_sync(std::collections::HashMap::new()).await.unwrap();

    // Verify AST was saved as CBOR in SQLite
    let manager = r2d2_sqlite::SqliteConnectionManager::file(&db_path);
    let pool = r2d2::Pool::builder().max_size(8).build(manager).unwrap();
    {
        let conn = pool.get().unwrap();
        conn.execute("INSERT OR IGNORE INTO users (username, password_hash, role, token_version, created_at) VALUES ('*', '', 'anonymous', 1, 0);", []).unwrap();
        conn.execute("INSERT INTO acls (username, verb, path_pattern, priority) VALUES ('*', 'ALLOW', '/**', 0);", []).unwrap();

        let ast_blob: Vec<u8> = conn.query_row(
            "SELECT ast_blob FROM pages WHERE url_path = '/tip';",
            [],
            |r| r.get(0),
        ).expect("ast_blob exists");
        assert!(!ast_blob.is_empty(), "CBOR blob must not be empty");

        let doc_ast = hematite::markdown::ast::DocAst::from_cbor(&ast_blob).expect("CBOR deserialization succeeds");
        assert_eq!(doc_ast.title, "Testing AST Elements");
        assert!(doc_ast.blocks.iter().any(|b| matches!(b, hematite::markdown::ast::BlockNode::Admonition { .. })));
    }

    // Set up template engine with theme override
    let template_engine = std::sync::Arc::new(hematite::template::TemplateEngine::with_theme_override(Some(&theme_dir)).unwrap());
    let auth_manager = std::sync::Arc::new(AuthManager::new("secret-key-12345678901234567890"));
    let initialized = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));

    // Compute build hash and set in system_meta
    let build_hash = hematite::crawler::reconciler::compute_build_hash(&temp_dir.join("themes"), "custom", &temp_dir.join("scripts"));
    writer.set_system_meta("current_build_hash".to_string(), build_hash.to_vec()).await.unwrap();

    let state = hematite::server::AppState {
        db_pool: pool.clone(),
        auth_manager,
        template_engine,
        crawler,
        writer: writer.clone(),
        initialized,
        content_root: content_dir.clone(),
        config: std::sync::Arc::new(config),
        theme_dir,
        reconciler_notify: std::sync::Arc::new(tokio::sync::Notify::new()),
    };

    let app = hematite::server::create_router(state);

    // Request the page via SSR and verify JIT render used the custom element template
    let req = axum::http::Request::builder()
        .uri("/tip")
        .body(axum::body::Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), axum::http::StatusCode::OK);

    let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let body_str = String::from_utf8_lossy(&body_bytes);

    // Verify custom element template output
    assert!(body_str.contains("custom-callout-override callout-tip"));
    assert!(body_str.contains("📢 Pro Tip"));
    assert!(body_str.contains("This is a test admonition block that should be rendered with the custom element template!"));

    let _ = fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_reconciliation_worker_background_render() {
    use std::fs;

    let temp_dir = std::env::temp_dir().join(format!(
        "hematite_test_reconciler_{}",
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    let content_dir = temp_dir.join("content");
    let theme_dir = temp_dir.join("themes/default");
    let element_templates_dir = theme_dir.join("templates/elements");
    let db_path = temp_dir.join("test.db");

    fs::create_dir_all(&content_dir).unwrap();
    fs::create_dir_all(&element_templates_dir).unwrap();

    fs::write(content_dir.join("index.md"), "# Home\n\n## Section 1\nWelcome to Hematite.").unwrap();

    let mut reg = PluginRegistry::new();
    hematite::plugins::register_all(&mut reg);
    let engine = MarkdownEngine::new(std::sync::Arc::new(reg));

    let (writer, _handle) = hematite::db::DbActor::spawn(&db_path, &content_dir, 64).unwrap();
    let script_engine = hematite::scripts::ScriptEngine::new(&temp_dir.join("scripts"));
    let config = hematite::config::HematiteConfig::default();

    let crawler = std::sync::Arc::new(hematite::crawler::Crawler::new(
        content_dir.clone(),
        engine,
        writer.clone(),
        script_engine.clone(),
        config.clone(),
    ));

    crawler.initial_sync(std::collections::HashMap::new()).await.unwrap();

    let manager = r2d2_sqlite::SqliteConnectionManager::file(&db_path);
    let pool = r2d2::Pool::builder().max_size(8).build(manager).unwrap();

    // Write a new heading element template override in theme
    fs::write(
        element_templates_dir.join("heading.html"),
        r#"<h{{ block.data.level }} class="reconciled-heading">{{ rendered_html | safe }}</h{{ block.data.level }}>"#,
    ).unwrap();

    let template_engine = std::sync::Arc::new(hematite::template::TemplateEngine::with_theme_override(Some(&theme_dir)).unwrap());
    let notify = std::sync::Arc::new(tokio::sync::Notify::new());

    // Spawn reconciler
    let reconciler = std::sync::Arc::new(hematite::crawler::reconciler::ReconciliationWorker::new(
        pool.clone(),
        writer.clone(),
        template_engine.clone(),
        script_engine,
        config.to_json_value(),
        notify.clone(),
    ));

    let rec_handle = tokio::spawn(async move {
        reconciler.run_loop().await;
    });

    // Recompute build hash and update system_meta
    let target_hash = hematite::crawler::reconciler::compute_build_hash(&temp_dir.join("themes"), "default", &temp_dir.join("scripts"));
    writer.set_system_meta("current_build_hash".to_string(), target_hash.to_vec()).await.unwrap();

    // Pulse notification to trigger reconciler
    notify.notify_waiters();

    // Wait briefly for reconciliation worker to update the page
    let mut converged = false;
    for _ in 0..20 {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        if let Ok(conn) = pool.get() {
            let row: Result<(Option<Vec<u8>>, String), _> = conn.query_row(
                "SELECT build_hash, parsed_html FROM pages WHERE url_path = '/';",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            );
            if let Ok((Some(bh), html)) = row {
                if bh == target_hash && html.contains("reconciled-heading") {
                    converged = true;
                    break;
                }
            }
        }
    }

    assert!(converged, "Reconciliation worker successfully updated page with new template override");
    rec_handle.abort();
    let _ = fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_invalidate_page_cache() {
    use std::fs;
    let temp_dir = std::env::temp_dir().join(format!("hematite_test_invalidate_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    let content_dir = temp_dir.join("content");
    let db_path = temp_dir.join("test.db");
    fs::create_dir_all(&content_dir).unwrap();

    let (writer, _handle) = hematite::db::DbActor::spawn(&db_path, &content_dir, 64).unwrap();
    let manager = r2d2_sqlite::SqliteConnectionManager::file(&db_path);
    let pool = r2d2::Pool::builder().max_size(4).build(manager).unwrap();

    let page1 = hematite::db::actor::PageRecord {
        url_path: "/doc1".to_string(),
        file_path: "doc1.md".to_string(),
        title: "Doc 1".to_string(),
        parent_path: "".to_string(),
        depth: 1,
        is_dir_index: false,
        content_hash: [1u8; 16],
        frontmatter_json: "{}".to_string(),
        parsed_html: "<p>Doc 1</p>".to_string(),
        raw_content: "Doc 1".to_string(),
        taxonomy: "".to_string(),
        links: vec![],
        ast_blob: vec![],
        build_hash: [2u8; 16],
    };
    let page2 = hematite::db::actor::PageRecord {
        url_path: "/doc2".to_string(),
        file_path: "doc2.md".to_string(),
        title: "Doc 2".to_string(),
        parent_path: "".to_string(),
        depth: 1,
        is_dir_index: false,
        content_hash: [3u8; 16],
        frontmatter_json: "{}".to_string(),
        parsed_html: "<p>Doc 2</p>".to_string(),
        raw_content: "Doc 2".to_string(),
        taxonomy: "".to_string(),
        links: vec![],
        ast_blob: vec![],
        build_hash: [4u8; 16],
    };
    writer.upsert_page(page1).await.unwrap();
    writer.upsert_page(page2).await.unwrap();

    let conn = pool.get().unwrap();

    // 1. Invalidate single page by url
    let affected = hematite::crawler::invalidate_page_cache(&conn, Some("/doc1")).unwrap();
    assert_eq!(affected, 1);

    let doc1_hashes: (Vec<u8>, Vec<u8>) = conn
        .query_row(
            "SELECT content_hash, build_hash FROM pages WHERE url_path = '/doc1';",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(doc1_hashes.0, vec![0u8; 16]);
    assert_eq!(doc1_hashes.1, vec![0u8; 16]);

    // doc2 should be unaffected
    let doc2_hashes: (Vec<u8>, Vec<u8>) = conn
        .query_row(
            "SELECT content_hash, build_hash FROM pages WHERE url_path = '/doc2';",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(doc2_hashes.0, vec![3u8; 16]);

    // 2. Invalidate all pages
    let affected_all = hematite::crawler::invalidate_page_cache(&conn, None).unwrap();
    assert_eq!(affected_all, 2);

    let doc2_hashes_after: (Vec<u8>, Vec<u8>) = conn
        .query_row(
            "SELECT content_hash, build_hash FROM pages WHERE url_path = '/doc2';",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(doc2_hashes_after.0, vec![0u8; 16]);
    assert_eq!(doc2_hashes_after.1, vec![0u8; 16]);

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_database_version_and_migration() {
    use hematite::db::migrations::{compare_semver, parse_semver, run_migrations, CURRENT_VERSION};
    use rusqlite::Connection;

    // Test semver parsing and comparison
    assert_eq!(parse_semver("v0.0.3"), Some((0, 0, 3)));
    assert_eq!(parse_semver("0.0.3"), Some((0, 0, 3)));
    assert_eq!(parse_semver("v1.10.2"), Some((1, 10, 2)));
    assert_eq!(compare_semver("v0.0.1", "v0.0.3"), std::cmp::Ordering::Less);
    assert_eq!(compare_semver("v0.0.3", "v0.0.3"), std::cmp::Ordering::Equal);
    assert_eq!(compare_semver("v0.1.0", "v0.0.9"), std::cmp::Ordering::Greater);

    // Test fresh DB initialization
    let mut conn = Connection::open_in_memory().unwrap();
    run_migrations(&mut conn).unwrap();

    let db_ver: String = conn
        .query_row("SELECT version FROM schema_version LIMIT 1;", [], |row| row.get(0))
        .unwrap();
    assert_eq!(db_ver, CURRENT_VERSION);

    // Verify all baseline tables exist
    let tables: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='table';")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();

    assert!(tables.contains(&"pages".to_string()));
    assert!(tables.contains(&"directories".to_string()));
    assert!(tables.contains(&"pages_fts".to_string()));
    assert!(tables.contains(&"page_links".to_string()));
    assert!(tables.contains(&"system_meta".to_string()));
    assert!(tables.contains(&"users".to_string()));
    assert!(tables.contains(&"acls".to_string()));
    assert!(tables.contains(&"schema_version".to_string()));

    // Verify ast_blob and build_hash exist in pages table
    assert!(conn.prepare("SELECT ast_blob, build_hash FROM pages LIMIT 1;").is_ok());

    // Test simulation of older version migration
    conn.execute("DELETE FROM schema_version;", []).unwrap();
    conn.execute(
        "INSERT INTO schema_version (version, migrated_at) VALUES ('v0.0.1', 1000);",
        [],
    ).unwrap();

    // Re-run migrations
    run_migrations(&mut conn).unwrap();

    let updated_ver: String = conn
        .query_row("SELECT version FROM schema_version LIMIT 1;", [], |row| row.get(0))
        .unwrap();
    assert_eq!(updated_ver, CURRENT_VERSION);
}
