use rusqlite::{params, Connection};
use std::path::Path;
use tracing::info;

pub const CURRENT_SCHEMA_VERSION: i32 = 3;

pub fn initialize_database(conn: &mut Connection) -> Result<(), rusqlite::Error> {
    // Enable WAL mode, synchronous=NORMAL, and foreign_keys=ON
    let _ = conn.pragma_update(None, "journal_mode", "WAL");
    let _ = conn.pragma_update(None, "synchronous", "NORMAL");
    let _ = conn.pragma_update(None, "foreign_keys", "ON");

    run_migrations(conn)?;
    Ok(())
}

pub fn open_connection<P: AsRef<Path>>(path: P) -> Result<Connection, rusqlite::Error> {
    let mut conn = Connection::open(path)?;
    initialize_database(&mut conn)?;
    Ok(conn)
}

pub fn run_migrations(conn: &mut Connection) -> Result<(), rusqlite::Error> {
    // Migration version table
    conn.execute(
        "CREATE TABLE IF NOT EXISTS schema_version (
            version INTEGER NOT NULL PRIMARY KEY
        );",
        [],
    )?;

    let mut current_version: i32 = conn
        .query_row("SELECT version FROM schema_version LIMIT 1;", [], |row| row.get(0))
        .unwrap_or(0);

    info!(current_version, target_version = CURRENT_SCHEMA_VERSION, "Checking database migrations");

    if current_version < 1 {
        migrate_v1(conn)?;
        current_version = 1;
        conn.execute("DELETE FROM schema_version;", [])?;
        conn.execute("INSERT INTO schema_version (version) VALUES (?1);", params![current_version])?;
        info!("Applied migration v1");
    }

    if current_version < 2 {
        migrate_v2(conn)?;
        current_version = 2;
        conn.execute("DELETE FROM schema_version;", [])?;
        conn.execute("INSERT INTO schema_version (version) VALUES (?1);", params![current_version])?;
        info!("Applied migration v2");
    }

    if current_version < 3 {
        migrate_v3(conn)?;
        current_version = 3;
        conn.execute("DELETE FROM schema_version;", [])?;
        conn.execute("INSERT INTO schema_version (version) VALUES (?1);", params![current_version])?;
        info!("Applied migration v3");
    }

    Ok(())
}

fn migrate_v3(conn: &mut Connection) -> Result<(), rusqlite::Error> {
    let tx = conn.transaction()?;

    // Add ast_blob and build_hash to pages if they do not exist
    let has_ast_blob: bool = tx
        .prepare("SELECT ast_blob FROM pages LIMIT 1;")
        .is_ok();
    if !has_ast_blob {
        tx.execute("ALTER TABLE pages ADD COLUMN ast_blob BLOB;", [])?;
    }

    let has_build_hash: bool = tx
        .prepare("SELECT build_hash FROM pages LIMIT 1;")
        .is_ok();
    if !has_build_hash {
        tx.execute("ALTER TABLE pages ADD COLUMN build_hash BLOB;", [])?;
    }

    // System metadata key-value table for tracking current_build_hash
    tx.execute(
        "CREATE TABLE IF NOT EXISTS system_meta (
            key TEXT PRIMARY KEY,
            value BLOB NOT NULL
        );",
        [],
    )?;

    tx.commit()?;
    Ok(())
}

fn migrate_v2(conn: &mut Connection) -> Result<(), rusqlite::Error> {
    let has_col: bool = conn
        .prepare("SELECT title FROM directories LIMIT 1;")
        .is_ok();
    if !has_col {
        let tx = conn.transaction()?;
        tx.execute("ALTER TABLE directories ADD COLUMN title TEXT;", [])?;
        tx.commit()?;
    }
    Ok(())
}

fn migrate_v1(conn: &mut Connection) -> Result<(), rusqlite::Error> {
    let tx = conn.transaction()?;

    tx.execute_batch(
        r#"
        -- Pages & Metadata
        CREATE TABLE IF NOT EXISTS pages (
            url_path TEXT PRIMARY KEY,          -- e.g. "/infrastructure/networking"
            file_path TEXT NOT NULL UNIQUE,     -- e.g. "infrastructure/networking.md"
            title TEXT NOT NULL,
            parent_path TEXT NOT NULL,          -- e.g. "/infrastructure"
            depth INTEGER NOT NULL,             -- 0 = root, 1 = /foo, 2 = /foo/bar
            is_dir_index INTEGER NOT NULL DEFAULT 0, -- 1 if explicitly backed by index.md
            content_hash BLOB NOT NULL,         -- 16 bytes (xxh3_128)
            frontmatter_json TEXT NOT NULL DEFAULT '{}',
            parsed_html TEXT,                   -- Pre-rendered HTML cache
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_pages_parent ON pages(parent_path);
        CREATE INDEX IF NOT EXISTS idx_pages_depth ON pages(depth);

        -- Virtual Directories (Folders with children, regardless of explicit page existence)
        CREATE TABLE IF NOT EXISTS directories (
            url_path TEXT PRIMARY KEY,
            parent_path TEXT NOT NULL,
            depth INTEGER NOT NULL,
            title TEXT
        );

        -- Full-Text Search (FTS5)
        CREATE VIRTUAL TABLE IF NOT EXISTS pages_fts USING fts5(
            url_path UNINDEXED,
            title,
            content,
            taxonomy,
            tokenize = 'porter unicode61'
        );

        -- Internal Link & Backlink Graph (for Obsidian link resolution)
        CREATE TABLE IF NOT EXISTS page_links (
            source_url TEXT NOT NULL,
            target_raw TEXT NOT NULL,           -- e.g. "Architecture" or "setup/firewall"
            resolved_url TEXT,                  -- Resolved /path if matched
            PRIMARY KEY (source_url, target_raw),
            FOREIGN KEY (source_url) REFERENCES pages(url_path) ON DELETE CASCADE
        );

        -- Users & Credentials
        CREATE TABLE IF NOT EXISTS users (
            username TEXT PRIMARY KEY,
            password_hash TEXT NOT NULL,
            role TEXT NOT NULL,
            token_version INTEGER NOT NULL DEFAULT 1,
            created_at INTEGER NOT NULL
        );

        -- Access Control Lists
        CREATE TABLE IF NOT EXISTS acls (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            username TEXT NOT NULL,
            verb TEXT NOT NULL, -- ALLOW / DENY
            path_pattern TEXT NOT NULL,  -- Glob syntax: e.g. "/secret/*", "/internal/**"
            priority INTEGER NOT NULL DEFAULT 0,
            FOREIGN KEY (username) REFERENCES users(username) ON DELETE CASCADE
        );
        "#,
    )?;

    tx.commit()?;
    Ok(())
}
