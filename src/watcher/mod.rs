use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc as std_mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Notify;
use tracing::{error, info};

use crate::crawler::reconciler::compute_build_hash;
use crate::crawler::Crawler;
use crate::db::actor::DbWriterHandle;

pub struct WatcherService {
    content_root: PathBuf,
    themes_root: PathBuf,
    scripts_root: PathBuf,
    active_theme: String,
    crawler: Crawler,
    writer: DbWriterHandle,
    notify: Arc<Notify>,
}

impl WatcherService {
    pub fn new(
        content_root: PathBuf,
        themes_root: PathBuf,
        scripts_root: PathBuf,
        active_theme: String,
        crawler: Crawler,
        writer: DbWriterHandle,
        notify: Arc<Notify>,
    ) -> Self {
        Self {
            content_root,
            themes_root,
            scripts_root,
            active_theme,
            crawler,
            writer,
            notify,
        }
    }

    pub fn start(self) -> Result<(), Box<dyn std::error::Error>> {
        let (std_tx, std_rx) = std_mpsc::channel();

        let mut watcher = RecommendedWatcher::new(
            move |res| {
                if let Ok(event) = res {
                    let _ = std_tx.send(event);
                }
            },
            Config::default(),
        )?;

        watcher.watch(&self.content_root, RecursiveMode::Recursive)?;
        info!(root = ?self.content_root, "Started file watcher for content");

        if self.themes_root.exists() {
            let _ = watcher.watch(&self.themes_root, RecursiveMode::Recursive);
            info!(root = ?self.themes_root, "Started file watcher for themes");
        }

        if self.scripts_root.exists() {
            let _ = watcher.watch(&self.scripts_root, RecursiveMode::Recursive);
            info!(root = ?self.scripts_root, "Started file watcher for scripts");
        }

        // Debounce loop (50ms debouncing)
        tokio::spawn(async move {
            let _watcher = watcher;
            let mut pending_content: HashMap<PathBuf, Instant> = HashMap::new();
            let mut pending_template_or_script: bool = false;
            let mut template_last_event = Instant::now();
            let debounce_duration = Duration::from_millis(50);

            loop {
                while let Ok(event) = std_rx.try_recv() {
                    match event.kind {
                        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) => {
                            for path in event.paths {
                                if path.starts_with(&self.content_root) {
                                    if path.extension().and_then(|s| s.to_str()) == Some("md") {
                                        pending_content.insert(path, Instant::now());
                                    }
                                } else if path.starts_with(&self.themes_root) || path.starts_with(&self.scripts_root) {
                                    pending_template_or_script = true;
                                    template_last_event = Instant::now();
                                }
                            }
                        }
                        _ => {}
                    }
                }

                // Process markdown content events
                let now = Instant::now();
                let mut ready = Vec::new();
                pending_content.retain(|path, time| {
                    if now.duration_since(*time) >= debounce_duration {
                        ready.push(path.clone());
                        false
                    } else {
                        true
                    }
                });

                for path in ready {
                    info!(path = ?path, "File change debounced, syncing");
                    if let Err(e) = self.crawler.sync_single_file(&path).await {
                        error!(path = ?path, error = %e, "Failed to sync file change");
                    }
                }

                // Process template/script changes
                if pending_template_or_script && now.duration_since(template_last_event) >= debounce_duration {
                    pending_template_or_script = false;
                    let new_hash = compute_build_hash(&self.themes_root, &self.active_theme, &self.scripts_root);
                    info!("Theme or script modified, updated current_build_hash");
                    let _ = self.writer.set_system_meta("current_build_hash".to_string(), new_hash.to_vec()).await;
                    self.notify.notify_waiters();
                }

                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        });

        Ok(())
    }
}
