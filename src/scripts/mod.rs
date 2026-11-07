use mlua::LuaSerdeExt;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use tracing::error;

/// Entity representation for a page passed to hooks
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageHookEntity {
    pub url_path: String,
    pub file_path: String,
    pub title: String,
    pub parent_path: String,
    pub depth: i32,
    pub is_dir_index: bool,
    pub frontmatter_json: String,
    pub parsed_html: String,
    pub raw_content: String,
    pub taxonomy: String,
    pub links: Vec<String>,
    #[serde(default)]
    pub ast_json: serde_json::Value,
}

/// Information provided to page hooks about directory, page context, and site configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageHookContext {
    pub all_pages: Vec<PageCatalogItem>,
    pub all_directories: Vec<DirectoryCatalogItem>,
    #[serde(default)]
    pub config: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageCatalogItem {
    pub url_path: String,
    pub file_path: String,
    pub title: String,
    pub parent_path: String,
    pub is_dir_index: bool,
}

/// Entity representation for a directory passed to hooks
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectoryHookEntity {
    pub url_path: String,
    pub parent_path: String,
    pub depth: i32,
    pub title: String,
    pub files: Vec<PageCatalogItem>,
    pub subdirectories: Vec<String>,
    pub index_metadata: Option<String>, // frontmatter_json of _index.md / index.md if present
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectoryCatalogItem {
    pub url_path: String,
    pub parent_path: String,
    pub depth: i32,
}

#[derive(Debug, Clone)]
pub struct ScriptEngine {
    scripts_root: PathBuf,
}

impl ScriptEngine {
    pub fn new<P: AsRef<Path>>(scripts_root: P) -> Self {
        Self {
            scripts_root: scripts_root.as_ref().to_path_buf(),
        }
    }

    /// Discover scripts in a directory sorted alphabetically by file name
    pub fn discover_scripts(&self, subpath: &str) -> Vec<PathBuf> {
        let dir = self.scripts_root.join(subpath);
        if !dir.is_dir() {
            return Vec::new();
        }

        let mut entries = Vec::new();
        if let Ok(rd) = fs::read_dir(dir) {
            for entry in rd.flatten() {
                let path = entry.path();
                if path.is_file() {
                    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
                    if ext == "lua" || ext == "rhai" {
                        entries.push(path);
                    }
                }
            }
        }

        entries.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
        entries
    }

    /// Execute file hooks in order for a page entity
    pub fn run_page_hooks(
        &self,
        mut entity: PageHookEntity,
        context: &PageHookContext,
    ) -> PageHookEntity {
        let scripts = self.discover_scripts("hooks/file");
        for script_path in scripts {
            let _script_name = script_path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            let ext = script_path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
            match ext.as_str() {
                "lua" => {
                    entity = self.run_lua_page_hook(&script_path, entity, context);
                }
                "rhai" => {
                    entity = self.run_rhai_page_hook(&script_path, entity, context);
                }
                _ => {}
            }
        }
        entity
    }

    /// Execute directory hooks in order for a directory entity
    pub fn run_directory_hooks(
        &self,
        mut entity: DirectoryHookEntity,
        context: &PageHookContext,
    ) -> DirectoryHookEntity {
        let scripts = self.discover_scripts("hooks/directory");
        for script_path in scripts {
            let _script_name = script_path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            let ext = script_path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
            match ext.as_str() {
                "lua" => {
                    entity = self.run_lua_dir_hook(&script_path, entity, context);
                }
                "rhai" => {
                    entity = self.run_rhai_dir_hook(&script_path, entity, context);
                }
                _ => {}
            }
        }
        entity
    }

    fn run_lua_page_hook(
        &self,
        script_path: &Path,
        entity: PageHookEntity,
        context: &PageHookContext,
    ) -> PageHookEntity {
        let code = match fs::read_to_string(script_path) {
            Ok(c) => c,
            Err(e) => {
                error!(script = ?script_path, error = %e, "Failed to read Lua page hook script");
                return entity;
            }
        };

        let lua = mlua::Lua::new();

        let res: Result<PageHookEntity, mlua::Error> = (|| {
            let globals = lua.globals();
            let entity_val: mlua::Value = lua.to_value(&entity)?;
            let ctx_val: mlua::Value = lua.to_value(context)?;
            let cfg_val: mlua::Value = lua.to_value(&context.config)?;

            globals.set("entity", entity_val)?;
            globals.set("context", ctx_val)?;
            globals.set("config", cfg_val)?;

            let ret: mlua::Value = lua.load(&code).eval()?;
            let final_val = if ret.is_nil() {
                globals.get("entity")?
            } else {
                ret
            };

            let updated: PageHookEntity = lua.from_value(final_val)?;
            Ok(updated)
        })();

        match res {
            Ok(updated) => updated,
            Err(e) => {
                error!(script = ?script_path, error = %e, "Error executing Lua page hook");
                entity
            }
        }
    }

    fn run_rhai_page_hook(
        &self,
        script_path: &Path,
        entity: PageHookEntity,
        context: &PageHookContext,
    ) -> PageHookEntity {
        let code = match fs::read_to_string(script_path) {
            Ok(c) => c,
            Err(e) => {
                error!(script = ?script_path, error = %e, "Failed to read Rhai page hook script");
                return entity;
            }
        };

        let engine = rhai::Engine::new();
        let entity_json = match serde_json::to_string(&entity) {
            Ok(j) => j,
            Err(e) => {
                error!(error = %e, "Failed to serialize page entity to json for Rhai");
                return entity;
            }
        };
        let context_json = match serde_json::to_string(context) {
            Ok(j) => j,
            Err(e) => {
                error!(error = %e, "Failed to serialize context to json for Rhai");
                return entity;
            }
        };

        let config_json = match serde_json::to_string(&context.config) {
            Ok(j) => j,
            Err(_) => "{}".to_string(),
        };

        let entity_dynamic: rhai::Dynamic = match engine.parse_json(&entity_json, true) {
            Ok(d) => d.into(),
            Err(e) => {
                error!(error = %e, "Failed to parse entity JSON into Rhai Dynamic");
                return entity;
            }
        };
        let context_dynamic: rhai::Dynamic = match engine.parse_json(&context_json, true) {
            Ok(d) => d.into(),
            Err(e) => {
                error!(error = %e, "Failed to parse context JSON into Rhai Dynamic");
                return entity;
            }
        };
        let config_dynamic: rhai::Dynamic = match engine.parse_json(&config_json, true) {
            Ok(d) => d.into(),
            Err(_) => rhai::Dynamic::UNIT,
        };

        let mut scope = rhai::Scope::new();
        scope.push("entity", entity_dynamic);
        scope.push("context", context_dynamic);
        scope.push("config", config_dynamic);

        let eval_res = engine.eval_with_scope::<rhai::Dynamic>(&mut scope, &code);
        match eval_res {
            Ok(ret) => {
                let final_dynamic = if ret.is_unit() {
                    scope.get_value::<rhai::Dynamic>("entity").unwrap_or(ret)
                } else {
                    ret
                };

                let json_val = match rhai::serde::from_dynamic::<serde_json::Value>(&final_dynamic) {
                    Ok(v) => v,
                    Err(e) => {
                        error!(error = %e, "Failed to deserialize Rhai result to JSON Value");
                        return entity;
                    }
                };

                match serde_json::from_value::<PageHookEntity>(json_val) {
                    Ok(upd) => upd,
                    Err(e) => {
                        error!(error = %e, "Failed to decode updated PageHookEntity from Rhai");
                        entity
                    }
                }
            }
            Err(e) => {
                error!(script = ?script_path, error = %e, "Error executing Rhai page hook");
                entity
            }
        }
    }

    fn run_lua_dir_hook(
        &self,
        script_path: &Path,
        entity: DirectoryHookEntity,
        context: &PageHookContext,
    ) -> DirectoryHookEntity {
        let code = match fs::read_to_string(script_path) {
            Ok(c) => c,
            Err(e) => {
                error!(script = ?script_path, error = %e, "Failed to read Lua dir hook script");
                return entity;
            }
        };

        let lua = mlua::Lua::new();

        let res: Result<DirectoryHookEntity, mlua::Error> = (|| {
            let globals = lua.globals();
            let entity_val: mlua::Value = lua.to_value(&entity)?;
            let ctx_val: mlua::Value = lua.to_value(context)?;
            let cfg_val: mlua::Value = lua.to_value(&context.config)?;

            globals.set("entity", entity_val)?;
            globals.set("context", ctx_val)?;
            globals.set("config", cfg_val)?;

            let ret: mlua::Value = lua.load(&code).eval()?;
            let final_val = if ret.is_nil() {
                globals.get("entity")?
            } else {
                ret
            };

            let updated: DirectoryHookEntity = lua.from_value(final_val)?;
            Ok(updated)
        })();

        match res {
            Ok(updated) => updated,
            Err(e) => {
                error!(script = ?script_path, error = %e, "Error executing Lua dir hook");
                entity
            }
        }
    }

    fn run_rhai_dir_hook(
        &self,
        script_path: &Path,
        entity: DirectoryHookEntity,
        context: &PageHookContext,
    ) -> DirectoryHookEntity {
        let code = match fs::read_to_string(script_path) {
            Ok(c) => c,
            Err(e) => {
                error!(script = ?script_path, error = %e, "Failed to read Rhai dir hook script");
                return entity;
            }
        };

        let engine = rhai::Engine::new();
        let entity_json = match serde_json::to_string(&entity) {
            Ok(j) => j,
            Err(e) => {
                error!(error = %e, "Failed to serialize dir entity to json for Rhai");
                return entity;
            }
        };
        let context_json = match serde_json::to_string(context) {
            Ok(j) => j,
            Err(e) => {
                error!(error = %e, "Failed to serialize context to json for Rhai");
                return entity;
            }
        };

        let config_json = match serde_json::to_string(&context.config) {
            Ok(j) => j,
            Err(_) => "{}".to_string(),
        };

        let entity_dynamic: rhai::Dynamic = match engine.parse_json(&entity_json, true) {
            Ok(d) => d.into(),
            Err(e) => {
                error!(error = %e, "Failed to parse dir entity JSON into Rhai Dynamic");
                return entity;
            }
        };
        let context_dynamic: rhai::Dynamic = match engine.parse_json(&context_json, true) {
            Ok(d) => d.into(),
            Err(e) => {
                error!(error = %e, "Failed to parse context JSON into Rhai Dynamic");
                return entity;
            }
        };
        let config_dynamic: rhai::Dynamic = match engine.parse_json(&config_json, true) {
            Ok(d) => d.into(),
            Err(_) => rhai::Dynamic::UNIT,
        };

        let mut scope = rhai::Scope::new();
        scope.push("entity", entity_dynamic);
        scope.push("context", context_dynamic);
        scope.push("config", config_dynamic);

        let eval_res = engine.eval_with_scope::<rhai::Dynamic>(&mut scope, &code);
        match eval_res {
            Ok(ret) => {
                let final_dynamic = if ret.is_unit() {
                    scope.get_value::<rhai::Dynamic>("entity").unwrap_or(ret)
                } else {
                    ret
                };

                let json_val = match rhai::serde::from_dynamic::<serde_json::Value>(&final_dynamic) {
                    Ok(v) => v,
                    Err(e) => {
                        error!(error = %e, "Failed to deserialize Rhai dir result to JSON Value");
                        return entity;
                    }
                };

                match serde_json::from_value::<DirectoryHookEntity>(json_val) {
                    Ok(upd) => upd,
                    Err(e) => {
                        error!(error = %e, "Failed to decode updated DirectoryHookEntity from Rhai");
                        entity
                    }
                }
            }
            Err(e) => {
                error!(script = ?script_path, error = %e, "Error executing Rhai dir hook");
                entity
            }
        }
    }
}
