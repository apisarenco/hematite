pub mod actor;
pub mod migrations;

use std::path::{Path, PathBuf};
use rusqlite::{params, Connection};
use tokio::sync::mpsc;
use tracing::info;

use crate::db::actor::{DbWriteCommand, DbWriterHandle, PageRecord};
use crate::db::migrations::initialize_database;

pub struct DbActor {
    conn: Connection,
    rx: mpsc::Receiver<DbWriteCommand>,
    content_root: PathBuf,
}

impl DbActor {
    pub fn spawn<P: AsRef<Path>, R: AsRef<Path>>(
        db_path: P,
        content_root: R,
        buffer_size: usize,
    ) -> Result<(DbWriterHandle, tokio::task::JoinHandle<()>), rusqlite::Error> {
        let mut conn = Connection::open(db_path)?;
        initialize_database(&mut conn)?;

        let (tx, rx) = mpsc::channel(buffer_size);
        let actor = DbActor {
            conn,
            rx,
            content_root: content_root.as_ref().to_path_buf(),
        };

        let handle = tokio::task::spawn_blocking(move || {
            actor.run();
        });

        Ok((DbWriterHandle::new(tx), handle))
    }

    pub fn run(mut self) {
        info!("SQLite Single-Writer Actor started");
        while let Some(cmd) = self.rx.blocking_recv() {
            match cmd {
                DbWriteCommand::UpsertPage { page, resp } => {
                    let res = self.handle_upsert_page(*page);
                    if let Some(resp) = resp {
                        let _ = resp.send(res.map_err(|e| e.to_string()));
                    }
                }
                DbWriteCommand::RemoveFile { file_path, resp } => {
                    let res = self.handle_remove_file(&file_path);
                    if let Some(resp) = resp {
                        let _ = resp.send(res.map_err(|e| e.to_string()));
                    }
                }
                DbWriteCommand::PruneDeletedPages { active_files, resp } => {
                    let res = self.handle_prune(&active_files);
                    if let Some(resp) = resp {
                        let _ = resp.send(res.map_err(|e| e.to_string()));
                    }
                }
                DbWriteCommand::RebuildDirectoryTree { resp } => {
                    let res = self.handle_rebuild_directories();
                    if let Some(resp) = resp {
                        let _ = resp.send(res.map_err(|e| e.to_string()));
                    }
                }
                DbWriteCommand::UpdateDirectoryTitle { url_path, title, resp } => {
                    let res = self.conn.execute(
                        "UPDATE directories SET title = ?1 WHERE url_path = ?2;",
                        params![title, url_path],
                    ).map(|_| ()).map_err(|e| e.to_string());
                    if let Some(resp) = resp {
                        let _ = resp.send(res);
                    }
                }
                DbWriteCommand::GetAllPagesForHooks { resp } => {
                    let res = self.handle_get_all_pages_for_hooks().map_err(|e| e.to_string());
                    let _ = resp.send(res);
                }
                DbWriteCommand::GetAllDirectoriesForHooks { resp } => {
                    let res = self.handle_get_all_directories_for_hooks().map_err(|e| e.to_string());
                    let _ = resp.send(res);
                }
                DbWriteCommand::UpdatePageFromHook { entity, resp } => {
                    let res = self.handle_update_page_from_hook(*entity).map_err(|e| e.to_string());
                    if let Some(resp) = resp {
                        let _ = resp.send(res);
                    }
                }
                DbWriteCommand::UpdatePageRenderedHtml {
                    url_path,
                    content_hash,
                    parsed_html,
                    build_hash,
                    resp,
                } => {
                    let res = self
                        .handle_update_page_rendered_html(&url_path, &content_hash, &parsed_html, &build_hash)
                        .map_err(|e| e.to_string());
                    if let Some(resp) = resp {
                        let _ = resp.send(res);
                    }
                }
                DbWriteCommand::SetSystemMeta { key, value, resp } => {
                    let res = self.handle_set_system_meta(&key, &value).map_err(|e| e.to_string());
                    if let Some(resp) = resp {
                        let _ = resp.send(res);
                    }
                }
                DbWriteCommand::ExecuteRaw { sql, resp } => {
                    let res = self.conn.execute(&sql, []).map_err(|e| e.to_string());
                    if let Some(resp) = resp {
                        let _ = resp.send(res);
                    }
                }
            }
        }
        info!("SQLite Single-Writer Actor stopped");
    }

    fn handle_upsert_page(&mut self, page: PageRecord) -> Result<(), rusqlite::Error> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        let tx = self.conn.transaction()?;

        // Upsert into pages
        tx.execute(
            r#"INSERT INTO pages (
                url_path, file_path, title, parent_path, depth,
                is_dir_index, content_hash, frontmatter_json,
                parsed_html, ast_blob, build_hash, created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?12)
            ON CONFLICT(url_path) DO UPDATE SET
                file_path = excluded.file_path,
                title = excluded.title,
                parent_path = excluded.parent_path,
                depth = excluded.depth,
                is_dir_index = excluded.is_dir_index,
                content_hash = excluded.content_hash,
                frontmatter_json = excluded.frontmatter_json,
                parsed_html = excluded.parsed_html,
                ast_blob = excluded.ast_blob,
                build_hash = excluded.build_hash,
                updated_at = excluded.updated_at"#,
            params![
                page.url_path,
                page.file_path,
                page.title,
                page.parent_path,
                page.depth,
                if page.is_dir_index { 1 } else { 0 },
                page.content_hash.as_slice(),
                page.frontmatter_json,
                page.parsed_html,
                page.ast_blob.as_slice(),
                page.build_hash.as_slice(),
                now,
            ],
        )?;

        // Update FTS
        tx.execute("DELETE FROM pages_fts WHERE url_path = ?1;", params![page.url_path])?;
        tx.execute(
            "INSERT INTO pages_fts (url_path, title, content, taxonomy) VALUES (?1, ?2, ?3, ?4);",
            params![page.url_path, page.title, page.raw_content, page.taxonomy],
        )?;

        // Update page links
        tx.execute("DELETE FROM page_links WHERE source_url = ?1;", params![page.url_path])?;
        for target in page.links {
            let _ = tx.execute(
                "INSERT OR IGNORE INTO page_links (source_url, target_raw, resolved_url) VALUES (?1, ?2, NULL);",
                params![page.url_path, target],
            );
        }

        tx.commit()?;
        self.resolve_links()?;
        self.handle_rebuild_directories()?;
        Ok(())
    }

    fn handle_remove_file(&mut self, file_path: &Path) -> Result<(), rusqlite::Error> {
        let rel_path = file_path
            .strip_prefix(&self.content_root)
            .unwrap_or(file_path)
            .to_string_lossy()
            .to_string();

        let tx = self.conn.transaction()?;

        // Get url_path before delete to clear FTS and links
        let url_path: Option<String> = tx
            .query_row(
                "SELECT url_path FROM pages WHERE file_path = ?1;",
                params![rel_path],
                |row| row.get(0),
            )
            .ok();

        if let Some(url) = url_path {
            tx.execute("DELETE FROM pages_fts WHERE url_path = ?1;", params![url])?;
            tx.execute("DELETE FROM pages WHERE url_path = ?1;", params![url])?;
        }

        tx.commit()?;
        self.handle_rebuild_directories()?;
        Ok(())
    }

    fn handle_prune(&mut self, active_files: &[String]) -> Result<usize, rusqlite::Error> {
        let mut count = 0;
        let mut to_delete = Vec::new();
        {
            let mut stmt = self.conn.prepare("SELECT url_path, file_path FROM pages;")?;
            let rows = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;

            for r in rows.flatten() {
                if !active_files.contains(&r.1) {
                    to_delete.push(r);
                }
            }
        }

        if !to_delete.is_empty() {
            let tx = self.conn.transaction()?;
            for (url, file) in &to_delete {
                tx.execute("DELETE FROM pages_fts WHERE url_path = ?1;", params![url])?;
                tx.execute("DELETE FROM pages WHERE url_path = ?1;", params![url])?;
                info!(url = %url, file = %file, "Pruned obsolete page");
                count += 1;
            }
            tx.commit()?;
            self.handle_rebuild_directories()?;
        }

        Ok(count)
    }

    pub fn handle_rebuild_directories(&mut self) -> Result<(), rusqlite::Error> {
        use std::collections::HashSet;
        let mut dirs = HashSet::new();

        {
            let mut stmt = self.conn.prepare("SELECT url_path FROM pages;")?;
            let urls: Vec<String> = stmt
                .query_map([], |row| row.get(0))?
                .flatten()
                .collect();

            for url in urls {
                let parts: Vec<&str> = url.trim_matches('/').split('/').collect();
                let mut curr = String::new();
                for (i, seg) in parts.iter().enumerate() {
                    if seg.is_empty() {
                        continue;
                    }
                    if i < parts.len() - 1 {
                        curr.push('/');
                        curr.push_str(seg);
                        let parent = if curr.rfind('/').unwrap_or(0) == 0 {
                            "/".to_string()
                        } else {
                            curr[..curr.rfind('/').unwrap()].to_string()
                        };
                        let depth = curr.matches('/').count() as i32;
                        dirs.insert((curr.clone(), parent, depth));
                    }
                }
            }
        }

        dirs.insert(("/".to_string(), "".to_string(), 0));

        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM directories;", [])?;
        {
            let mut insert_stmt = tx.prepare(
                "INSERT OR IGNORE INTO directories (url_path, parent_path, depth) VALUES (?1, ?2, ?3);",
            )?;

            for (dir_path, parent_path, depth) in dirs {
                insert_stmt.execute(params![dir_path, parent_path, depth])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    fn resolve_links(&mut self) -> Result<(), rusqlite::Error> {
        let page_rows: Vec<(String, String, String)>;
        let unresolved: Vec<(String, String)>;

        {
            let mut stmt = self.conn.prepare("SELECT url_path, file_path, title FROM pages;")?;
            page_rows = stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                .flatten()
                .collect();
        }

        {
            let mut unresolved_stmt = self.conn.prepare("SELECT source_url, target_raw FROM page_links;")?;
            unresolved = unresolved_stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .flatten()
                .collect();
        }

        let tx = self.conn.transaction()?;
        {
            let mut update_stmt = tx.prepare(
                "UPDATE page_links SET resolved_url = ?1 WHERE source_url = ?2 AND target_raw = ?3;",
            )?;

            for (source_url, target_raw) in unresolved {
                let resolved = resolve_target(&source_url, &target_raw, &page_rows);
                if let Some((res, _)) = resolved {
                    let _ = update_stmt.execute(params![res, source_url, target_raw]);
                }
            }
        }
        tx.commit()?;
        Ok(())
    }

    fn handle_get_all_pages_for_hooks(&self) -> Result<Vec<crate::scripts::PageHookEntity>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT url_path, file_path, title, parent_path, depth, is_dir_index, frontmatter_json, parsed_html, ast_blob
             FROM pages ORDER BY url_path ASC;",
        )?;

        let rows = stmt.query_map([], |r| {
            let url_path: String = r.get(0)?;
            let file_path: String = r.get(1)?;
            let title: String = r.get(2)?;
            let parent_path: String = r.get(3)?;
            let depth: i32 = r.get(4)?;
            let is_dir_index: bool = r.get::<_, i32>(5)? == 1;
            let frontmatter_json: String = r.get(6)?;
            let parsed_html: String = r.get::<_, Option<String>>(7)?.unwrap_or_default();
            let ast_blob: Option<Vec<u8>> = r.get(8)?;
            Ok((url_path, file_path, title, parent_path, depth, is_dir_index, frontmatter_json, parsed_html, ast_blob))
        })?;

        let mut pages = Vec::new();
        for r in rows.flatten() {
            let (url_path, file_path, title, parent_path, depth, is_dir_index, frontmatter_json, parsed_html, ast_blob) = r;
            // Fetch links for page
            let mut links = Vec::new();
            if let Ok(mut l_stmt) = self.conn.prepare("SELECT target_raw FROM page_links WHERE source_url = ?1;") {
                if let Ok(l_rows) = l_stmt.query_map(params![url_path], |lr| lr.get(0)) {
                    for target in l_rows.flatten() {
                        links.push(target);
                    }
                }
            }

            let ast_json = if let Some(bytes) = ast_blob {
                if let Ok(ast) = crate::markdown::ast::DocAst::from_cbor(&bytes) {
                    serde_json::to_value(ast).unwrap_or(serde_json::Value::Null)
                } else {
                    serde_json::Value::Null
                }
            } else {
                serde_json::Value::Null
            };

            pages.push(crate::scripts::PageHookEntity {
                url_path,
                file_path,
                title,
                parent_path,
                depth,
                is_dir_index,
                frontmatter_json,
                parsed_html,
                raw_content: String::new(),
                taxonomy: String::new(),
                links,
                ast_json,
            });
        }

        Ok(pages)
    }

    fn handle_get_all_directories_for_hooks(&self) -> Result<Vec<crate::scripts::DirectoryHookEntity>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT d.url_path, d.parent_path, d.depth, COALESCE(d.title, p.title, REPLACE(SUBSTR(d.url_path, INSTR(d.url_path, '/') + 1), '-', ' ')), p.frontmatter_json
             FROM directories d
             LEFT JOIN pages p ON p.url_path = d.url_path AND p.is_dir_index = 1
             ORDER BY d.depth ASC, d.url_path ASC;",
        )?;

        let dir_rows: Vec<(String, String, i32, String, Option<String>)> = stmt
            .query_map([], |r| {
                let u: String = r.get(0)?;
                let parent: String = r.get(1)?;
                let depth: i32 = r.get(2)?;
                let title: String = r.get(3)?;
                let index_meta: Option<String> = r.get(4)?;
                let name = if title.is_empty() {
                    u.split('/').last().unwrap_or(&u).replace('-', " ")
                } else {
                    title
                };
                Ok((u, parent, depth, name, index_meta))
            })?
            .flatten()
            .collect();

        let mut result = Vec::new();
        for (dir_path, parent_path, depth, title, index_metadata) in dir_rows {
            // Find files in directory
            let mut files = Vec::new();
            if let Ok(mut f_stmt) = self.conn.prepare("SELECT url_path, file_path, title, parent_path, is_dir_index FROM pages WHERE parent_path = ?1;") {
                if let Ok(f_rows) = f_stmt.query_map(params![dir_path], |fr| {
                    Ok(crate::scripts::PageCatalogItem {
                        url_path: fr.get(0)?,
                        file_path: fr.get(1)?,
                        title: fr.get(2)?,
                        parent_path: fr.get(3)?,
                        is_dir_index: fr.get::<_, i32>(4)? == 1,
                    })
                }) {
                    for f in f_rows.flatten() {
                        files.push(f);
                    }
                }
            }

            // Find immediate subdirectories
            let mut subdirectories = Vec::new();
            if let Ok(mut s_stmt) = self.conn.prepare("SELECT url_path FROM directories WHERE parent_path = ?1 AND url_path != ?1;") {
                if let Ok(s_rows) = s_stmt.query_map(params![dir_path], |sr| sr.get(0)) {
                    for s in s_rows.flatten() {
                        subdirectories.push(s);
                    }
                }
            }

            result.push(crate::scripts::DirectoryHookEntity {
                url_path: dir_path,
                parent_path,
                depth,
                title,
                files,
                subdirectories,
                index_metadata,
            });
        }

        Ok(result)
    }

    fn handle_update_page_from_hook(&mut self, entity: crate::scripts::PageHookEntity) -> Result<(), rusqlite::Error> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        let tx = self.conn.transaction()?;
        tx.execute(
            "UPDATE pages SET title = ?1, frontmatter_json = ?2, parsed_html = ?3, updated_at = ?4 WHERE url_path = ?5;",
            params![entity.title, entity.frontmatter_json, entity.parsed_html, now, entity.url_path],
        )?;

        // Update FTS
        tx.execute("UPDATE pages_fts SET title = ?1 WHERE url_path = ?2;", params![entity.title, entity.url_path])?;

        tx.commit()?;
        Ok(())
    }

    fn handle_update_page_rendered_html(
        &mut self,
        url_path: &str,
        content_hash: &[u8; 16],
        parsed_html: &str,
        build_hash: &[u8; 16],
    ) -> Result<bool, rusqlite::Error> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        // OCC check: only update if content_hash matches!
        let updated = self.conn.execute(
            "UPDATE pages 
             SET parsed_html = ?1, build_hash = ?2, updated_at = ?3 
             WHERE url_path = ?4 AND content_hash = ?5;",
            params![
                parsed_html,
                build_hash.as_slice(),
                now,
                url_path,
                content_hash.as_slice(),
            ],
        )?;

        Ok(updated > 0)
    }

    fn handle_set_system_meta(&mut self, key: &str, value: &[u8]) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "INSERT INTO system_meta (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value;",
            params![key, value],
        )?;
        Ok(())
    }
}

pub fn resolve_target(
    source_url: &str,
    target_raw: &str,
    pages: &[(String, String, String)],
) -> Option<(String, String)> {
    let clean_raw = target_raw
        .strip_suffix(".md")
        .or_else(|| target_raw.strip_suffix(".markdown"))
        .unwrap_or(target_raw);

    let clean_target = if clean_raw.starts_with('/') {
        crate::crawler::normalize_slug_path(clean_raw)
    } else {
        crate::crawler::normalize_slug_path(&format!("/{}", clean_raw.trim_matches('/')))
    };

    for (url, _, title) in pages {
        if url == &clean_target || url.trim_end_matches('/') == clean_target.trim_end_matches('/') {
            return Some((url.clone(), title.clone()));
        }
    }

    // 2. Relative sibling match
    let source_dir = if let Some(idx) = source_url.rfind('/') {
        &source_url[..idx]
    } else {
        ""
    };
    let sibling_candidate = crate::crawler::normalize_slug_path(&format!("{source_dir}/{}", clean_raw.trim_matches('/')));
    for (url, _, title) in pages {
        if url == &sibling_candidate {
            return Some((url.clone(), title.clone()));
        }
    }

    // 3. Exact filename match (e.g. "Session 14-s.md" or "Session 14-s" matching file_path basename)
    let target_filename = target_raw
        .split('/')
        .last()
        .unwrap_or(target_raw);
    let target_stem = target_filename
        .strip_suffix(".md")
        .or_else(|| target_filename.strip_suffix(".markdown"))
        .unwrap_or(target_filename)
        .trim();

    // Check case-insensitive exact filename/stem match
    for (url, file_path, title) in pages {
        let path_obj = Path::new(file_path);
        let actual_stem = path_obj.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        let actual_file_name = path_obj.file_name().and_then(|s| s.to_str()).unwrap_or("");

        if actual_stem.eq_ignore_ascii_case(target_stem)
            || actual_file_name.eq_ignore_ascii_case(target_filename)
        {
            return Some((url.clone(), title.clone()));
        }
    }

    // 4. Slug / normalized filename match (e.g. [[Architecture]] matching docs/system/Architecture.md)
    let target_slug = crate::crawler::slugify_segment(target_stem).to_lowercase();

    let mut matches = Vec::new();
    for (url, file_path, title) in pages {
        let fname = Path::new(file_path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        let fname_slug = crate::crawler::slugify_segment(fname).to_lowercase();
        if fname_slug == target_slug {
            matches.push((url.clone(), title.clone()));
        }
    }

    // Prefer shortest path depth
    matches.sort_by_key(|(u, _)| u.matches('/').count());
    matches.into_iter().next()
}
