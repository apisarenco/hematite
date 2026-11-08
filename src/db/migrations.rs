use rusqlite::{params, Connection};
use std::cmp::Ordering;
use std::path::Path;
use tracing::info;

/// Current software version embedded at compile time (e.g. "v0.0.3").
pub const CURRENT_VERSION: &str = env!("HEMATITE_VERSION");

/// Migration step definition.
struct Migration {
    /// The software version this migration upgrades from.
    from_version: &'static str,
    /// The software version this migration upgrades to.
    to_version: &'static str,
    /// The migration execution logic.
    run: fn(&mut Connection) -> Result<(), rusqlite::Error>,
}

/// Registered sequential migrations.
/// Note: Initial baseline DDL initializes the database up to the status quo at v0.0.3.
/// Future migrations are appended here as versions evolve (e.g. from "v0.0.3" to "v0.0.4").
static MIGRATIONS: &[Migration] = &[
    // Example future migration:
    // Migration {
    //     from_version: "v0.0.3",
    //     to_version: "v0.0.4",
    //     run: migrate_v0_0_3_to_v0_0_4,
    // },
];

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

/// Parse semantic version string like "v0.0.3" or "0.0.3" into (major, minor, patch).
pub fn parse_semver(v: &str) -> Option<(u64, u64, u64)> {
    let s = v.trim().trim_start_matches(|c| c == 'v' || c == 'V');
    let mut parts = s.split('.');
    let major = parts.next()?.parse::<u64>().ok()?;
    let minor = parts.next()?.parse::<u64>().ok()?;
    let patch = parts.next()?.parse::<u64>().ok()?;
    Some((major, minor, patch))
}

pub fn compare_semver(a: &str, b: &str) -> Ordering {
    match (parse_semver(a), parse_semver(b)) {
        (Some(sa), Some(sb)) => sa.cmp(&sb),
        _ => a.cmp(b),
    }
}

pub fn run_migrations(conn: &mut Connection) -> Result<(), rusqlite::Error> {
    // Schema version table storing the migrated semantic version string
    conn.execute(
        "CREATE TABLE IF NOT EXISTS schema_version (
            version TEXT NOT NULL PRIMARY KEY,
            migrated_at INTEGER NOT NULL
        );",
        [],
    )?;

    // Check last migrated version from DB
    let last_version: Option<String> = conn
        .query_row(
            "SELECT version FROM schema_version ORDER BY ROWID DESC LIMIT 1;",
            [],
            |row| row.get(0),
        )
        .ok();

    match last_version {
        None => {
            // Fresh database: apply the complete status quo DDL directly
            info!(target_version = CURRENT_VERSION, "Fresh database detected, applying base schema");
            apply_baseline_schema(conn)?;
            set_migrated_version(conn, CURRENT_VERSION)?;
            info!(version = CURRENT_VERSION, "Base schema applied successfully");
        }
        Some(ref last) if last != CURRENT_VERSION => {
            info!(
                from_version = last.as_str(),
                to_version = CURRENT_VERSION,
                "Database version differs from software version, checking migrations"
            );

            let mut active_version = last.clone();

            for migration in MIGRATIONS {
                // If the database is at migration.from_version or older and migration targets <= CURRENT_VERSION
                if compare_semver(&active_version, migration.from_version) == Ordering::Equal
                    || (compare_semver(&active_version, migration.from_version) == Ordering::Less
                        && compare_semver(migration.to_version, CURRENT_VERSION) != Ordering::Greater)
                {
                    info!(
                        from = migration.from_version,
                        to = migration.to_version,
                        "Running migration"
                    );
                    (migration.run)(conn)?;
                    active_version = migration.to_version.to_string();
                    set_migrated_version(conn, &active_version)?;
                    info!(version = %active_version, "Completed migration step");
                }
            }

            // Finally update version in DB to current software version
            set_migrated_version(conn, CURRENT_VERSION)?;
            info!(
                previous_version = last.as_str(),
                current_version = CURRENT_VERSION,
                "Database version successfully updated to software version"
            );
        }
        Some(ref last) => {
            info!(version = last.as_str(), "Database schema is up to date");
        }
    }

    Ok(())
}

fn set_migrated_version(conn: &mut Connection, version: &str) -> Result<(), rusqlite::Error> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    let tx = conn.transaction()?;
    tx.execute("DELETE FROM schema_version;", [])?;
    tx.execute(
        "INSERT INTO schema_version (version, migrated_at) VALUES (?1, ?2);",
        params![version, now],
    )?;
    tx.commit()?;
    Ok(())
}

/// Baseline DDL representing the current status quo of Hematite
fn apply_baseline_schema(conn: &mut Connection) -> Result<(), rusqlite::Error> {
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
            ast_blob BLOB,                      -- Serialized CBOR DocAst
            build_hash BLOB,                    -- 16 bytes (xxh3_128) build hash for cache validation
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

        -- System metadata key-value table for tracking current_build_hash and state
        CREATE TABLE IF NOT EXISTS system_meta (
            key TEXT PRIMARY KEY,
            value BLOB NOT NULL
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
