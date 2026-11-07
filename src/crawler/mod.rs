pub mod reconciler;

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{error, info};
use walkdir::WalkDir;
use xxhash_rust::xxh3::xxh3_128;

use crate::config::HematiteConfig;
use crate::db::actor::{DbWriterHandle, PageRecord};
use crate::markdown::MarkdownEngine;
use crate::scripts::{
    DirectoryCatalogItem, PageCatalogItem, PageHookContext, ScriptEngine,
};

#[derive(Clone)]
pub struct Crawler {
    content_root: PathBuf,
    engine: MarkdownEngine,
    writer: DbWriterHandle,
    script_engine: ScriptEngine,
    config: HematiteConfig,
}

impl Crawler {
    pub fn new(
        content_root: PathBuf,
        engine: MarkdownEngine,
        writer: DbWriterHandle,
        script_engine: ScriptEngine,
        config: HematiteConfig,
    ) -> Self {
        Self {
            content_root,
            engine,
            writer,
            script_engine,
            config,
        }
    }

    /// Perform initial crawl and index synchronization
    pub async fn initial_sync(&self, existing_hashes: HashMap<String, Vec<u8>>) -> Result<(), Box<dyn std::error::Error>> {
        info!(root = ?self.content_root, "Starting initial filesystem crawler sync");

        // 1. First pass: Collect all markdown and media files, and compute metadata
        let mut active_files = Vec::new();
        let mut file_entries = Vec::new();
        let mut page_catalog: Vec<(String, String, String)> = Vec::new(); // (url_path, rel_file_path, title)
        let mut media_catalog: HashMap<String, String> = HashMap::new(); // (filename.to_lowercase() -> url_encoded_rel_path)

        for entry in WalkDir::new(&self.content_root).into_iter().flatten() {
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path().to_path_buf();
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();

            let rel_path = match path.strip_prefix(&self.content_root) {
                Ok(rp) => rp.to_string_lossy().to_string(),
                Err(_) => continue,
            };

            if ext == "md" {
                active_files.push(rel_path.clone());
                let (url_path, _, _, _, default_title) = derive_path_metadata(&rel_path);

                // Read title upfront from frontmatter / H1 / default_title
                let title = match fs::read_to_string(&path) {
                    Ok(content) => crate::markdown::extract_title_from_raw(&content, &default_title),
                    Err(_) => default_title,
                };

                page_catalog.push((url_path, rel_path.clone(), title));
                file_entries.push((path, rel_path));
            } else if matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "gif" | "svg" | "webp" | "bmp" | "ico") {
                let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
                let encoded_rel_path = rel_path
                    .split('/')
                    .map(|seg| urlencoding::encode(seg).into_owned())
                    .collect::<Vec<_>>()
                    .join("/");
                media_catalog.insert(file_name.to_lowercase(), encoded_rel_path.clone());
                media_catalog.insert(rel_path.to_lowercase(), encoded_rel_path);
            }
        }

        let resolver = |source_url: &str, target_raw: &str| -> (String, Option<String>) {
            if let Some((resolved_url, resolved_title)) = crate::db::resolve_target(source_url, target_raw, &page_catalog) {
                (resolved_url, Some(resolved_title))
            } else {
                (crate::crawler::normalize_slug_path(target_raw), None)
            }
        };

        let media_resolver = |target_raw: &str| -> Option<String> {
            let key = target_raw.trim_matches('/').to_lowercase();
            media_catalog.get(&key).map(|rel| format!("/_media/{}", rel))
        };

        // 2. Process files and diff hashes
        let mut updated_count = 0;
        let mut unchanged_count = 0;
        let mut batch_records = Vec::new();

        for (path, rel_path) in file_entries {
            let bytes = match fs::read(&path) {
                Ok(b) => b,
                Err(e) => {
                    error!(path = ?path, error = %e, "Failed to read file");
                    continue;
                }
            };

            let hash_u128 = xxh3_128(&bytes);
            let hash_bytes = hash_u128.to_be_bytes();

            if let Some(existing) = existing_hashes.get(&rel_path) {
                if existing.as_slice() == hash_bytes {
                    unchanged_count += 1;
                    continue; // Untouched! Zero-lag skip
                }
            }

            // File is new or changed: parse with context-aware link resolver
            let raw_str = String::from_utf8_lossy(&bytes);
            let (url_path, parent_path, depth, is_dir_index, default_title) = derive_path_metadata(&rel_path);

            let parsed = self.engine.parse_with_context(
                &raw_str,
                &default_title,
                Some(&url_path),
                Some(&resolver),
                Some(&media_resolver),
            );

            let mut fm_val: serde_json::Value = serde_json::from_str(&parsed.frontmatter_json)
                .unwrap_or(serde_json::Value::Object(Default::default()));
            if let Some(obj) = fm_val.as_object_mut() {
                obj.insert("title_source".to_string(), serde_json::Value::String(parsed.title_source.clone()));
            }
            let updated_fm_json = serde_json::to_string(&fm_val).unwrap_or(parsed.frontmatter_json);

            let record = PageRecord {
                url_path,
                file_path: rel_path,
                title: parsed.title,
                parent_path,
                depth,
                is_dir_index,
                content_hash: hash_bytes,
                frontmatter_json: updated_fm_json,
                parsed_html: parsed.html,
                raw_content: parsed.raw_content,
                taxonomy: String::new(),
                links: parsed.links,
                ast_blob: parsed.ast_blob,
                build_hash: [0u8; 16],
            };

            batch_records.push(record);
            updated_count += 1;

            if batch_records.len() >= 50 {
                let chunk = std::mem::take(&mut batch_records);
                self.writer.upsert_pages_batch(chunk).await.map_err(|e| e)?;
            }
        }

        if !batch_records.is_empty() {
            self.writer.upsert_pages_batch(batch_records).await.map_err(|e| e)?;
        }

        // 3. Prune deleted records
        let pruned = self.writer.prune_deleted_pages(active_files).await.map_err(|e| e)?;

        // 4. Rebuild directory hierarchy
        self.writer.rebuild_directory_tree().await.map_err(|e| e)?;

        // 5. Run Extensibility Hooks (Page hooks first, then Directory hooks)
        self.run_hooks().await?;

        info!(
            updated = updated_count,
            unchanged = unchanged_count,
            pruned = pruned,
            "Initial filesystem sync complete"
        );

        Ok(())
    }

    /// Execute page and directory hooks
    pub async fn run_hooks(&self) -> Result<(), Box<dyn std::error::Error>> {
        // Fetch current pages and directories from database
        let pages = self.writer.get_all_pages_for_hooks().await.map_err(|e| e)?;
        let directories = self.writer.get_all_directories_for_hooks().await.map_err(|e| e)?;

        let all_pages: Vec<PageCatalogItem> = pages
            .iter()
            .map(|p| PageCatalogItem {
                url_path: p.url_path.clone(),
                file_path: p.file_path.clone(),
                title: p.title.clone(),
                parent_path: p.parent_path.clone(),
                is_dir_index: p.is_dir_index,
            })
            .collect();

        let all_directories: Vec<DirectoryCatalogItem> = directories
            .iter()
            .map(|d| DirectoryCatalogItem {
                url_path: d.url_path.clone(),
                parent_path: d.parent_path.clone(),
                depth: d.depth,
            })
            .collect();

        let context = PageHookContext {
            all_pages,
            all_directories,
            config: self.config.to_json_value(),
        };

        // 1. Run Page Hooks
        for page in pages {
            let orig_title = page.title.clone();
            let orig_frontmatter = page.frontmatter_json.clone();
            let orig_html = page.parsed_html.clone();

            let modified = self.script_engine.run_page_hooks(page, &context);

            if modified.title != orig_title
                || modified.frontmatter_json != orig_frontmatter
                || modified.parsed_html != orig_html
            {
                self.writer.update_page_from_hook(modified).await.map_err(|e| e)?;
            }
        }

        // 2. Run Directory Hooks
        for dir in directories {
            let orig_title = dir.title.clone();
            let modified = self.script_engine.run_directory_hooks(dir, &context);
            if modified.title != orig_title {
                self.writer.update_directory_title(modified.url_path, modified.title).await.map_err(|e| e)?;
            }
        }

        Ok(())
    }

    /// Process a single file update or insertion on demand (e.g. from file watcher or boot fallback)
    pub async fn sync_single_file(&self, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let rel_path = path
            .strip_prefix(&self.content_root)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string();

        if !path.exists() {
            self.writer.remove_file(path.to_path_buf()).await.map_err(|e| e)?;
            return Ok(());
        }

        let bytes = fs::read(path)?;
        let hash_bytes = xxh3_128(&bytes).to_be_bytes();
        let raw_str = String::from_utf8_lossy(&bytes);
        let (url_path, parent_path, depth, is_dir_index, default_title) = derive_path_metadata(&rel_path);

        // Build quick catalog for link and media resolution
        let mut page_catalog = Vec::new();
        let mut media_catalog: HashMap<String, String> = HashMap::new();
        for entry in WalkDir::new(&self.content_root).into_iter().flatten() {
            if entry.file_type().is_file() {
                let p = entry.path();
                let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
                if let Ok(rp) = p.strip_prefix(&self.content_root) {
                    let rp_str = rp.to_string_lossy().to_string();
                    if ext == "md" {
                        let (u, _, _, _, default_title) = derive_path_metadata(&rp_str);
                        let title = match fs::read_to_string(p) {
                            Ok(content) => crate::markdown::extract_title_from_raw(&content, &default_title),
                            Err(_) => default_title,
                        };
                        page_catalog.push((u, rp_str, title));
                    } else if matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "gif" | "svg" | "webp" | "bmp" | "ico") {
                        let file_name = p.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
                        let encoded_rel_path = rp_str
                            .split('/')
                            .map(|seg| urlencoding::encode(seg).into_owned())
                            .collect::<Vec<_>>()
                            .join("/");
                        media_catalog.insert(file_name.to_lowercase(), encoded_rel_path.clone());
                        media_catalog.insert(rp_str.to_lowercase(), encoded_rel_path);
                    }
                }
            }
        }

        let resolver = |source_url: &str, target_raw: &str| -> (String, Option<String>) {
            if let Some((resolved_url, resolved_title)) = crate::db::resolve_target(source_url, target_raw, &page_catalog) {
                (resolved_url, Some(resolved_title))
            } else {
                (crate::crawler::normalize_slug_path(target_raw), None)
            }
        };

        let media_resolver = |target_raw: &str| -> Option<String> {
            let key = target_raw.trim_matches('/').to_lowercase();
            media_catalog.get(&key).map(|rel| format!("/_media/{}", rel))
        };

        let parsed = self.engine.parse_with_context(
            &raw_str,
            &default_title,
            Some(&url_path),
            Some(&resolver),
            Some(&media_resolver),
        );

        let mut fm_val: serde_json::Value = serde_json::from_str(&parsed.frontmatter_json)
            .unwrap_or(serde_json::Value::Object(Default::default()));
        if let Some(obj) = fm_val.as_object_mut() {
            obj.insert("title_source".to_string(), serde_json::Value::String(parsed.title_source.clone()));
        }
        let updated_fm_json = serde_json::to_string(&fm_val).unwrap_or(parsed.frontmatter_json);

        let record = PageRecord {
            url_path,
            file_path: rel_path,
            title: parsed.title,
            parent_path,
            depth,
            is_dir_index,
            content_hash: hash_bytes,
            frontmatter_json: updated_fm_json,
            parsed_html: parsed.html,
            raw_content: parsed.raw_content,
            taxonomy: String::new(),
            links: parsed.links,
            ast_blob: parsed.ast_blob,
            build_hash: [0u8; 16],
        };

        self.writer.upsert_page(record).await.map_err(|e| e)?;
        self.run_hooks().await?;
        Ok(())
    }
}

/// Normalizes a single path segment using regex s/[^a-zA-Z0-9-]+/-/
pub fn slugify_segment(seg: &str) -> String {
    let mut out = String::with_capacity(seg.len());
    let mut last_was_dash = false;

    for c in seg.chars() {
        if c.is_ascii_alphanumeric() || c == '-' {
            out.push(c);
            last_was_dash = c == '-';
        } else if !last_was_dash {
            out.push('-');
            last_was_dash = true;
        }
    }

    // Trim trailing/leading dashes if any
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        seg.to_string()
    } else {
        trimmed.to_string()
    }
}

/// Normalizes a URL path by slugifying each segment with s/[^a-zA-Z0-9-]+/-/
pub fn normalize_slug_path(path: &str) -> String {
    let trimmed = path.trim_matches('/');
    if trimmed.is_empty() {
        return "/".to_string();
    }
    let parts: Vec<String> = trimmed
        .split('/')
        .filter(|s| !s.is_empty())
        .map(slugify_segment)
        .collect();

    if parts.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", parts.join("/"))
    }
}

/// Computes (url_path, parent_path, depth, is_dir_index, default_title) from a relative markdown path
pub fn derive_path_metadata(rel_path: &str) -> (String, String, i32, bool, String) {
    let normalized = rel_path.replace('\\', "/");
    let trimmed = normalized.strip_suffix(".md").unwrap_or(&normalized);

    let parts: Vec<&str> = trimmed.split('/').filter(|s| !s.is_empty()).collect();

    if parts.is_empty() {
        return ("/".to_string(), "".to_string(), 0, false, "Home".to_string());
    }

    let last = parts[parts.len() - 1];
    let is_dir_index = last.eq_ignore_ascii_case("index")
        || last.eq_ignore_ascii_case("_index")
        || last.eq_ignore_ascii_case("readme");

    let raw_url_parts: Vec<&str> = if is_dir_index {
        parts[..parts.len() - 1].to_vec()
    } else {
        parts.clone()
    };

    let url_parts: Vec<String> = raw_url_parts.iter().map(|s| slugify_segment(s)).collect();

    let url_path = if url_parts.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", url_parts.join("/"))
    };

    let parent_path = if url_path == "/" {
        "".to_string()
    } else if let Some(last_slash) = url_path.rfind('/') {
        if last_slash == 0 {
            "/".to_string()
        } else {
            url_path[..last_slash].to_string()
        }
    } else {
        "/".to_string()
    };

    let depth = if url_path == "/" {
        0
    } else {
        url_path.matches('/').count() as i32
    };

    let default_title = if is_dir_index && raw_url_parts.is_empty() {
        "Home".to_string()
    } else if is_dir_index && !raw_url_parts.is_empty() {
        titlecase(raw_url_parts.last().unwrap())
    } else {
        titlecase(last)
    };

    (url_path, parent_path, depth, is_dir_index, default_title)
}

/// Invalidate page cache in SQLite, setting content_hash and build_hash to zero
/// so that subsequent crawler indexing or request JIT re-parses and re-renders the pages.
pub fn invalidate_page_cache(
    conn: &rusqlite::Connection,
    target_path: Option<&str>,
) -> Result<usize, rusqlite::Error> {
    match target_path {
        Some(target) => {
            conn.execute(
                "UPDATE pages SET content_hash = zeroblob(16), build_hash = zeroblob(16) 
                 WHERE url_path = ?1 OR file_path = ?1 OR url_path LIKE ?2 OR file_path LIKE ?2;",
                rusqlite::params![target, format!("%{}%", target)],
            )
        }
        None => {
            let affected = conn.execute(
                "UPDATE pages SET content_hash = zeroblob(16), build_hash = zeroblob(16);",
                [],
            )?;
            let _ = conn.execute(
                "UPDATE system_meta SET value = zeroblob(16) WHERE key = 'current_build_hash';",
                [],
            );
            Ok(affected)
        }
    }
}

fn titlecase(s: &str) -> String {
    let clean = s.replace('-', " ").replace('_', " ");
    let mut words = Vec::new();
    for w in clean.split_whitespace() {
        let mut chars = w.chars();
        if let Some(first) = chars.next() {
            let capitalized = format!("{}{}", first.to_uppercase(), chars.as_str());
            words.push(capitalized);
        }
    }
    words.join(" ")
}
