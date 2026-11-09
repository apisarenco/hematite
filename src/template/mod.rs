use minijinja::{Environment, Error as JinjaError};
use std::fs;
use std::path::Path;
use tracing::info;

pub mod renderer;

pub struct TemplateEngine {
    env: Environment<'static>,
}

impl TemplateEngine {
    pub fn env(&self) -> &Environment<'static> {
        &self.env
    }
    pub fn new() -> Result<Self, JinjaError> {
        Self::with_theme_override(None)
    }

    pub fn with_theme_override(theme_dir: Option<&Path>) -> Result<Self, JinjaError> {
        let mut env = Environment::new();

        // Custom filters
        env.add_filter("urlencode", |value: &str| -> String {
            urlencoding::encode(value).into_owned()
        });

        // Register embedded default templates
        env.add_template("page.html", include_str!("../../templates/page.html"))?;
        env.add_template(
            "directory_index.html",
            include_str!("../../templates/directory_index.html"),
        )?;
        env.add_template(
            "components/directory_content.html",
            include_str!("../../templates/components/directory_content.html"),
        )?;
        env.add_template(
            "components/nav_tree.html",
            include_str!("../../templates/components/nav_tree.html"),
        )?;
        env.add_template(
            "components/breadcrumbs.html",
            include_str!("../../templates/components/breadcrumbs.html"),
        )?;
        env.add_template(
            "components/error_content.html",
            include_str!("../../templates/components/error_content.html"),
        )?;
        env.add_template("404.html", include_str!("../../templates/404.html"))?;
        env.add_template("403.html", include_str!("../../templates/403.html"))?;
        env.add_template("400.html", include_str!("../../templates/400.html"))?;
        env.add_template("login.html", include_str!("../../templates/login.html"))?;
        env.add_template("admin.html", include_str!("../../templates/admin.html"))?;

        // If theme directory exists, override templates with theme files
        if let Some(td) = theme_dir {
            let tmpl_root = td.join("templates");
            if tmpl_root.is_dir() {
                Self::load_templates_from_dir(&mut env, &tmpl_root, "")?;
            }
        }

        Ok(Self { env })
    }

    fn load_templates_from_dir(env: &mut Environment<'static>, dir: &Path, prefix: &str) -> Result<(), JinjaError> {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                let file_name = entry.file_name().to_string_lossy().to_string();
                let name = if prefix.is_empty() {
                    file_name.clone()
                } else {
                    format!("{}/{}", prefix, file_name)
                };

                if path.is_dir() {
                    Self::load_templates_from_dir(env, &path, &name)?;
                } else if path.is_file() && file_name.ends_with(".html") {
                    if let Ok(content) = fs::read_to_string(&path) {
                        info!(template_name = %name, path = ?path, "Loaded theme template override");
                        // Box::leak allows giving static lifetime to dynamically loaded theme templates
                        let leaked: &'static str = Box::leak(content.into_boxed_str());
                        let name_leaked: &'static str = Box::leak(name.into_boxed_str());
                        let _ = env.add_template(name_leaked, leaked);
                    }
                }
            }
        }
        Ok(())
    }

    pub fn render<S: serde::Serialize>(&self, template_name: &str, ctx: S) -> Result<String, JinjaError> {
        let tmpl = self.env.get_template(template_name)?;
        tmpl.render(ctx)
    }
}
