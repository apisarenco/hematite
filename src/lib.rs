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

