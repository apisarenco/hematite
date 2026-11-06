use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HematiteConfig {
    #[serde(default = "default_name")]
    pub name: String,

    #[serde(default = "default_base_url")]
    pub base_url: String,

    #[serde(default = "default_theme")]
    pub theme: String,

    #[serde(flatten, default)]
    pub extra: HashMap<String, serde_json::Value>,
}

fn default_name() -> String {
    "Hematite".to_string()
}

fn default_base_url() -> String {
    "/".to_string()
}

fn default_theme() -> String {
    "default".to_string()
}

impl Default for HematiteConfig {
    fn default() -> Self {
        Self {
            name: default_name(),
            base_url: default_base_url(),
            theme: default_theme(),
            extra: HashMap::new(),
        }
    }
}

impl HematiteConfig {
    /// Load configuration from a specific path, or search default locations.
    /// Default search order:
    /// 1. Provided explicit path (if Some)
    /// 2. `./hematite.yaml` or `./hematite.yml`
    /// 3. `<content_dir>/../hematite.yaml`
    /// 4. `<content_dir>/hematite.yaml`
    pub fn load_or_default(explicit_path: Option<&Path>, content_dir: Option<&Path>) -> Self {
        if let Some(path) = explicit_path {
            if path.exists() {
                match Self::from_file(path) {
                    Ok(cfg) => {
                        info!(path = ?path, "Loaded configuration from explicit path");
                        return cfg;
                    }
                    Err(e) => {
                        warn!(path = ?path, error = %e, "Failed to parse explicit configuration file; using defaults");
                    }
                }
            } else {
                warn!(path = ?path, "Explicit configuration file does not exist; checking defaults");
            }
        }

        let mut candidate_paths = vec![
            PathBuf::from("hematite.yaml"),
            PathBuf::from("hematite.yml"),
        ];

        if let Some(cd) = content_dir {
            if let Some(parent) = cd.parent() {
                candidate_paths.push(parent.join("hematite.yaml"));
                candidate_paths.push(parent.join("hematite.yml"));
            }
            candidate_paths.push(cd.join("hematite.yaml"));
            candidate_paths.push(cd.join("hematite.yml"));
        }

        for candidate in candidate_paths {
            if candidate.exists() {
                match Self::from_file(&candidate) {
                    Ok(cfg) => {
                        info!(path = ?candidate, "Loaded configuration from discovered path");
                        return cfg;
                    }
                    Err(e) => {
                        warn!(path = ?candidate, error = %e, "Failed to parse discovered configuration file; trying next");
                    }
                }
            }
        }

        info!("No hematite.yaml found; using default configuration");
        Self::default()
    }

    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self, Box<dyn std::error::Error>> {
        let content = fs::read_to_string(path)?;
        let config: Self = serde_yaml::from_str(&content)?;
        Ok(config)
    }

    pub fn from_yaml_str(yaml_str: &str) -> Result<Self, serde_yaml::Error> {
        serde_yaml::from_str(yaml_str)
    }

    /// Convert config to a serde_json::Value representation (useful for templates and scripts)
    pub fn to_json_value(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or_else(|_| {
            serde_json::json!({
                "name": self.name,
                "base_url": self.base_url,
                "theme": self.theme,
            })
        })
    }
}
