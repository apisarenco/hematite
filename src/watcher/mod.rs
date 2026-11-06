use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc as std_mpsc;
use std::time::{Duration, Instant};
use tracing::{error, info};

use crate::crawler::Crawler;

pub struct WatcherService {
    content_root: PathBuf,
    crawler: Crawler,
}

impl WatcherService {
    pub fn new(content_root: PathBuf, crawler: Crawler) -> Self {
        Self {
            content_root,
            crawler,
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
        info!(root = ?self.content_root, "Started file watcher");

        // Debounce loop (50ms debouncing)
        tokio::spawn(async move {
            // Keep watcher alive
            let _watcher = watcher;
            let mut pending_events: HashMap<PathBuf, Instant> = HashMap::new();
            let debounce_duration = Duration::from_millis(50);

            loop {
                // Read any available events without blocking indefinitely
                while let Ok(event) = std_rx.try_recv() {
                    match event.kind {
                        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) => {
                            for path in event.paths {
                                if path.extension().and_then(|s| s.to_str()) == Some("md") {
                                    pending_events.insert(path, Instant::now());
                                }
                            }
                        }
                        _ => {}
                    }
                }

                // Process events whose debounce timer expired
                let now = Instant::now();
                let mut ready = Vec::new();
                pending_events.retain(|path, time| {
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

                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        });

        Ok(())
    }
}
