use std::path::Path;
use std::sync::Arc;
use tokio::sync::Notify;
use tracing::info;
use walkdir::WalkDir;
use xxhash_rust::xxh3::xxh3_128;

use crate::db::actor::DbWriterHandle;
use crate::markdown::ast::DocAst;
use crate::scripts::{PageHookContext, PageHookEntity, ScriptEngine};
use crate::template::renderer::ElementRenderer;
use crate::template::TemplateEngine;

/// Computes the composite hash of templates and scripts
pub fn compute_build_hash(themes_dir: &Path, active_theme: &str, scripts_dir: &Path) -> [u8; 16] {
    let mut all_bytes = Vec::new();

    // Include theme templates if present
    let theme_path = themes_dir.join(active_theme).join("templates");
    if theme_path.exists() {
        for entry in WalkDir::new(&theme_path).into_iter().flatten() {
            if entry.file_type().is_file() {
                if let Ok(bytes) = std::fs::read(entry.path()) {
                    all_bytes.extend_from_slice(entry.path().to_string_lossy().as_bytes());
                    all_bytes.extend_from_slice(&bytes);
                }
            }
        }
    }

    // Include scripts
    if scripts_dir.exists() {
        for entry in WalkDir::new(scripts_dir).into_iter().flatten() {
            if entry.file_type().is_file() {
                if let Ok(bytes) = std::fs::read(entry.path()) {
                    all_bytes.extend_from_slice(entry.path().to_string_lossy().as_bytes());
                    all_bytes.extend_from_slice(&bytes);
                }
            }
        }
    }

    if all_bytes.is_empty() {
        // Fallback salt
        all_bytes.extend_from_slice(b"hematite-default-build-hash");
    }

    xxh3_128(&all_bytes).to_be_bytes()
}

pub struct ReconciliationWorker {
    db_pool: r2d2::Pool<r2d2_sqlite::SqliteConnectionManager>,
    writer: DbWriterHandle,
    template_engine: Arc<TemplateEngine>,
    script_engine: ScriptEngine,
    config: serde_json::Value,
    notify: Arc<Notify>,
}

impl ReconciliationWorker {
    pub fn new(
        db_pool: r2d2::Pool<r2d2_sqlite::SqliteConnectionManager>,
        writer: DbWriterHandle,
        template_engine: Arc<TemplateEngine>,
        script_engine: ScriptEngine,
        config: serde_json::Value,
        notify: Arc<Notify>,
    ) -> Self {
        Self {
            db_pool,
            writer,
            template_engine,
            script_engine,
            config,
            notify,
        }
    }

    /// Background task execution loop
    pub async fn run_loop(self: Arc<Self>) {
        info!("Reconciliation worker started");

        let renderer = ElementRenderer::new(self.template_engine.clone());

        loop {
            // Check for outdated pages
            let current_build_hash = self.get_current_build_hash();
            if let Some(target_hash) = current_build_hash {
                let outdated_batch = self.fetch_outdated_pages(&target_hash, 20);

                if !outdated_batch.is_empty() {
                    let context = PageHookContext {
                        all_pages: Vec::new(),
                        all_directories: Vec::new(),
                        config: self.config.clone(),
                    };

                    for (url_path, content_hash, ast_blob) in outdated_batch {
                        if let Ok(mut ast) = DocAst::from_cbor(&ast_blob) {
                            // Run page hooks if any
                            let entity = PageHookEntity {
                                url_path: url_path.clone(),
                                file_path: String::new(),
                                title: ast.title.clone(),
                                parent_path: String::new(),
                                depth: 0,
                                is_dir_index: false,
                                frontmatter_json: ast.frontmatter.to_string(),
                                parsed_html: String::new(),
                                raw_content: String::new(),
                                taxonomy: String::new(),
                                links: ast.links.clone(),
                                ast_json: serde_json::to_value(&ast).unwrap_or(serde_json::Value::Null),
                            };

                            let modified = self.script_engine.run_page_hooks(entity, &context);
                            if let Ok(updated_ast) = serde_json::from_value::<DocAst>(modified.ast_json) {
                                ast = updated_ast;
                            }
                            if modified.title != ast.title {
                                ast.title = modified.title;
                            }

                            // Render with element renderer
                            let rendered_html = renderer.render_doc(&ast);

                            // Send OCC-guarded update to DbActor
                            let _ = self
                                .writer
                                .update_page_rendered_html(url_path, content_hash, rendered_html, target_hash)
                                .await;
                        }
                    }

                    // Yield execution between batches
                    tokio::task::yield_now().await;
                    continue;
                }
            }

            // Idle wait until notified of a template or script change
            self.notify.notified().await;
        }
    }

    fn get_current_build_hash(&self) -> Option<[u8; 16]> {
        if let Ok(conn) = self.db_pool.get() {
            let res: Result<Vec<u8>, _> = conn.query_row(
                "SELECT value FROM system_meta WHERE key = 'current_build_hash';",
                [],
                |row| row.get(0),
            );
            if let Ok(bytes) = res {
                if bytes.len() == 16 {
                    let mut arr = [0u8; 16];
                    arr.copy_from_slice(&bytes);
                    return Some(arr);
                }
            }
        }
        None
    }

    fn fetch_outdated_pages(
        &self,
        current_hash: &[u8; 16],
        limit: usize,
    ) -> Vec<(String, [u8; 16], Vec<u8>)> {
        let mut results = Vec::new();
        if let Ok(conn) = self.db_pool.get() {
            if let Ok(mut stmt) = conn.prepare(
                "SELECT url_path, content_hash, ast_blob 
                 FROM pages 
                 WHERE ast_blob IS NOT NULL 
                   AND (build_hash IS NULL OR build_hash != ?1) 
                 LIMIT ?2;",
            ) {
                if let Ok(rows) = stmt.query_map(rusqlite::params![current_hash.as_slice(), limit], |r| {
                    let url: String = r.get(0)?;
                    let c_hash_vec: Vec<u8> = r.get(1)?;
                    let ast_blob: Vec<u8> = r.get(2)?;
                    let mut c_hash = [0u8; 16];
                    if c_hash_vec.len() == 16 {
                        c_hash.copy_from_slice(&c_hash_vec);
                    }
                    Ok((url, c_hash, ast_blob))
                }) {
                    for row in rows.flatten() {
                        results.push(row);
                    }
                }
            }
        }
        results
    }
}
