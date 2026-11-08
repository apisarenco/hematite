use clap::{Parser, Subcommand};
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tracing::info;

pub mod auth;
pub mod config;
pub mod crawler;
pub mod db;
pub mod markdown;
pub mod plugin;
pub mod plugins;
pub mod scripts;
pub mod server;
pub mod template;
pub mod watcher;

/// Current software version embedded at compile time (e.g. "v0.0.3").
pub const VERSION: &str = env!("HEMATITE_VERSION");

#[derive(Parser, Debug)]
#[command(name = "hematite", author, version = VERSION, about = "High-Performance Rust Markdown CMS")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    #[arg(short, long, default_value = "./content", env = "HEMATITE_CONTENT_DIR")]
    content: PathBuf,

    #[arg(long, default_value = "./scripts", env = "HEMATITE_SCRIPTS_DIR")]
    scripts: PathBuf,

    #[arg(short, long, default_value = "hematite.db", env = "HEMATITE_DB_PATH")]
    database: PathBuf,

    #[arg(short = 'C', long, env = "HEMATITE_CONFIG_PATH")]
    config: Option<PathBuf>,

    #[arg(long, default_value = "./themes", env = "HEMATITE_THEMES_DIR")]
    themes: PathBuf,

    #[arg(short, long, default_value = "0.0.0.0:3000", env = "HEMATITE_BIND_ADDR")]
    bind: SocketAddr,

    #[arg(long, default_value = "supersecretkey12345678901234567890", env = "HEMATITE_JWT_SECRET")]
    jwt_secret: String,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Invalidate page cache to force a full re-parse and reconciliation
    InvalidateCache {
        #[arg(short, long, default_value = "hematite.db", env = "HEMATITE_DB_PATH")]
        database: Option<PathBuf>,

        /// Invalidate a specific page by url_path or file_path pattern (omit to invalidate all pages)
        #[arg(short, long)]
        path: Option<String>,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();

    // Handle subcommands
    if let Some(Commands::InvalidateCache { database, path }) = cli.command {
        let db_path = database.unwrap_or(cli.database);
        if !db_path.exists() {
            eprintln!("Error: Database file does not exist at {}", db_path.display());
            return Ok(());
        }
        println!("Invalidating page cache in database: {}", db_path.display());

        let mut conn = rusqlite::Connection::open(&db_path)?;
        let _ = conn.pragma_update(None, "journal_mode", "WAL");
        let _ = conn.pragma_update(None, "synchronous", "NORMAL");
        let _ = conn.pragma_update(None, "busy_timeout", 5000);

        // Ensure schema is up to date before querying
        let _ = db::migrations::run_migrations(&mut conn);

        let count = match crawler::invalidate_page_cache(&conn, path.as_deref()) {
            Ok(affected) => {
                if let Some(ref target) = path {
                    println!("Successfully invalidated {affected} page(s) matching '{target}'.");
                } else {
                    println!("Successfully invalidated all {affected} page(s).");
                }
                affected
            }
            Err(e) => {
                eprintln!("Error invalidating page cache: {e}");
                return Err(Box::<dyn std::error::Error>::from(e));
            }
        };

        if count == 0 {
            eprintln!("Warning: No matching pages were found in the database.");
            return Ok(());
        }

        // Trigger automatic sweep immediately
        println!("Starting automatic crawler sweep across content directory...");
        let config = Arc::new(config::HematiteConfig::load_or_default(
            cli.config.as_deref(),
            Some(&cli.content),
        ));
        let (writer_handle, _actor_join) = db::DbActor::spawn(&db_path, &cli.content, 256)?;
        let mut plugin_registry = plugin::PluginRegistry::new();
        plugins::register_all(&mut plugin_registry);
        let md_engine = markdown::MarkdownEngine::new(Arc::new(plugin_registry));
        let script_engine = scripts::ScriptEngine::new(&cli.scripts);
        let crawler = crawler::Crawler::new(
            cli.content.clone(),
            md_engine,
            writer_handle.clone(),
            script_engine,
            (*config).clone(),
        );

        let existing_hashes = {
            let mut map = std::collections::HashMap::new();
            if let Ok(mut stmt) = conn.prepare("SELECT file_path, content_hash FROM pages;") {
                if let Ok(rows) = stmt.query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
                }) {
                    for r in rows.flatten() {
                        map.insert(r.0, r.1);
                    }
                }
            }
            map
        };

        if let Err(e) = crawler.initial_sync(existing_hashes).await {
            eprintln!("Automatic sweep failed: {e}");
            return Err(e);
        }

        // Recompute build hash and update system_meta
        let target_build_hash = crawler::reconciler::compute_build_hash(&cli.themes, &config.theme, &cli.scripts);
        let _ = writer_handle.set_system_meta("current_build_hash".to_string(), target_build_hash.to_vec()).await;

        println!("Automatic sweep completed successfully. All invalidated pages have been re-parsed and persisted.");
        return Ok(());
    }

    info!("Starting Hematite CMS daemon...");

    // 0. Load Configuration (hematite.yaml)
    let config = Arc::new(config::HematiteConfig::load_or_default(
        cli.config.as_deref(),
        Some(&cli.content),
    ));
    info!(
        name = %config.name,
        base_url = %config.base_url,
        theme = %config.theme,
        "Site configuration initialized"
    );

    // Create content directory if it doesn't exist
    if !cli.content.exists() {
        std::fs::create_dir_all(&cli.content)?;
        // Write default index.md
        let sample_index = r#"---
title: Welcome to Hematite
tags: [welcome, rust]
---

# Welcome to Hematite

A high-performance Rust Markdown CMS engine.

Check out [[Features]] or visit the [Admin panel](/_admin/).
"#;
        let _ = std::fs::write(cli.content.join("index.md"), sample_index);
        let sample_features = r#"---
title: Features
---

# Features of Hematite

- **Sub-millisecond SSR**
- **Obsidian Wikilinks** support like [[index|Home Page]]
- **Stateless JWT ACLs** with token versions
- **SQLite WAL Mode** with single writer
"#;
        let _ = std::fs::write(cli.content.join("features.md"), sample_features);
    }

    // 1. Initialize SQLite connection pool for readers
    let manager = SqliteConnectionManager::file(&cli.database);
    let pool = Pool::builder().max_size(32).build(manager)?;

    // 2. Initialize DB writer actor
    let (writer_handle, _actor_join) = db::DbActor::spawn(&cli.database, &cli.content, 256)?;

    // 3. Initialize plugin registry & markdown engine
    let mut plugin_registry = plugin::PluginRegistry::new();
    plugins::register_all(&mut plugin_registry);
    let registry_arc = Arc::new(plugin_registry);
    let md_engine = markdown::MarkdownEngine::new(registry_arc);

    // 4. Initialize template engine & auth
    let theme_dir = cli.themes.join(&config.theme);
    let template_engine = Arc::new(template::TemplateEngine::with_theme_override(Some(&theme_dir))?);
    let auth_manager = Arc::new(auth::AuthManager::new(&cli.jwt_secret));
    let initialized = Arc::new(AtomicBool::new(false));

    // Ensure default admin user & default public ACL exists
    {
        let conn = pool.get()?;
        let has_admin: bool = conn
            .query_row(
                "SELECT 1 FROM users WHERE username = 'admin';",
                [],
                |_| Ok(true),
            )
            .unwrap_or(false);

        if !has_admin {
            let admin_hash = auth::AuthManager::hash_password("admin123")?;
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;
            conn.execute(
                "INSERT INTO users (username, password_hash, role, token_version, created_at)
                 VALUES ('admin', ?1, 'admin', 1, ?2);",
                rusqlite::params![admin_hash, now],
            )?;

            // Ensure wildcard user exists for public ACL rule
            conn.execute(
                "INSERT OR IGNORE INTO users (username, password_hash, role, token_version, created_at)
                 VALUES ('*', '', 'anonymous', 1, ?1);",
                rusqlite::params![now],
            )?;

            // Default ACL for admin: allow all
            conn.execute(
                "INSERT INTO acls (username, verb, path_pattern, priority) VALUES ('admin', 'ALLOW', '/**', 100);",
                [],
            )?;
            // Default ACL for public: allow all content & public endpoints
            conn.execute(
                "INSERT INTO acls (username, verb, path_pattern, priority) VALUES ('*', 'ALLOW', '/**', 0);",
                [],
            )?;
            info!("Default admin user created: admin / admin123");
        }
    }

    // 5. Initialize script engine, reconciler notify, and crawler
    let reconciler_notify = Arc::new(tokio::sync::Notify::new());
    let script_engine = scripts::ScriptEngine::new(&cli.scripts);
    let crawler = Arc::new(crawler::Crawler::new(
        cli.content.clone(),
        md_engine,
        writer_handle.clone(),
        script_engine.clone(),
        (*config).clone(),
    ));

    // Save initial build hash in system_meta
    let initial_build_hash = crawler::reconciler::compute_build_hash(&cli.themes, &config.theme, &cli.scripts);
    let _ = writer_handle.set_system_meta("current_build_hash".to_string(), initial_build_hash.to_vec()).await;

    // Spawn background reconciliation worker
    let reconciler = Arc::new(crawler::reconciler::ReconciliationWorker::new(
        pool.clone(),
        writer_handle.clone(),
        template_engine.clone(),
        script_engine,
        config.to_json_value(),
        reconciler_notify.clone(),
    ));
    tokio::spawn(async move {
        reconciler.run_loop().await;
    });

    // Perform initial filesystem crawl in background task or immediately
    let crawler_clone = crawler.clone();
    let pool_clone = pool.clone();
    let init_flag = initialized.clone();
    tokio::spawn(async move {
        let existing_hashes = {
            let mut map = std::collections::HashMap::new();
            if let Ok(conn) = pool_clone.get() {
                if let Ok(mut stmt) = conn.prepare("SELECT file_path, content_hash FROM pages;") {
                    if let Ok(rows) = stmt.query_map([], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
                    }) {
                        for r in rows.flatten() {
                            map.insert(r.0, r.1);
                        }
                    }
                }
            }
            map
        };

        if let Err(e) = crawler_clone.initial_sync(existing_hashes).await {
            tracing::error!(error = %e, "Initial crawler sync failed");
        } else {
            init_flag.store(true, Ordering::Release);
            tracing::info!("Filesystem crawler initial sync completed");
        }
    });

    // 6. Start file watcher
    let watcher_service = watcher::WatcherService::new(
        cli.content.clone(),
        cli.themes.clone(),
        cli.scripts.clone(),
        config.theme.clone(),
        (*crawler).clone(),
        writer_handle.clone(),
        reconciler_notify.clone(),
    );
    if let Err(e) = watcher_service.start() {
        tracing::error!(error = %e, "Failed to initialize watcher service");
    }

    // 7. Start Axum HTTP server
    let state = server::AppState {
        db_pool: pool,
        auth_manager,
        template_engine,
        crawler,
        writer: writer_handle,
        initialized,
        content_root: cli.content.clone(),
        config: config.clone(),
        theme_dir: theme_dir.clone(),
        reconciler_notify: reconciler_notify.clone(),
    };

    let router = server::create_router(state);
    info!("Binding HTTP server on {}", cli.bind);
    let listener = tokio::net::TcpListener::bind(cli.bind).await?;
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    info!("Hematite server shut down successfully");
    std::process::exit(0);
}

async fn shutdown_signal() {
    use tokio::signal;

    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {
            tracing::info!("Received SIGINT (Ctrl+C), shutting down immediately");
        },
        _ = terminate => {
            tracing::info!("Received SIGTERM, shutting down immediately");
        },
    }
}
