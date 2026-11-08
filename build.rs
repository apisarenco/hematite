use std::fs;
use std::path::Path;
use std::process::Command;

fn determine_version() -> String {
    // 1. If explicit env var is set (e.g. HEMATITE_VERSION or APP_VERSION), use it
    if let Ok(val) = std::env::var("HEMATITE_VERSION") {
        if !val.trim().is_empty() {
            return normalize_version(val.trim());
        }
    }
    if let Ok(val) = std::env::var("APP_VERSION") {
        if !val.trim().is_empty() {
            return normalize_version(val.trim());
        }
    }

    // 2. Try git describe --tags --match "v[0-9]*" --abbrev=0
    let git_desc = Command::new("git")
        .args(["describe", "--tags", "--match", "v[0-9]*", "--abbrev=0"])
        .output();
    if let Ok(output) = git_desc {
        if output.status.success() {
            let tag = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !tag.is_empty() {
                return normalize_version(&tag);
            }
        }
    }

    // 3. Fallback to Cargo.toml package version
    let cargo_pkg_ver = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".to_string());
    normalize_version(&cargo_pkg_ver)
}

fn normalize_version(ver: &str) -> String {
    let trimmed = ver.trim();
    if trimmed.starts_with('v') || trimmed.starts_with('V') {
        format!("v{}", &trimmed[1..])
    } else {
        format!("v{}", trimmed)
    }
}

fn main() {
    println!("cargo:rerun-if-changed=src/plugins");
    println!("cargo:rerun-if-env-changed=HEMATITE_VERSION");
    println!("cargo:rerun-if-env-changed=APP_VERSION");
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs");

    let version = determine_version();
    println!("cargo:rustc-env=HEMATITE_VERSION={}", version);

    let plugins_dir = Path::new("src/plugins");
    let mut mod_decls = String::new();
    let mut reg_calls = String::new();

    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();

    if plugins_dir.exists() {
        for entry in fs::read_dir(plugins_dir).unwrap().flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("rs") {
                let name = path.file_stem().unwrap().to_str().unwrap();
                if name != "mod" {
                    let abs_file = format!("{manifest_dir}/src/plugins/{name}.rs");
                    mod_decls.push_str(&format!(
                        "#[path = \"{abs_file}\"]\npub mod {name};\n"
                    ));
                    reg_calls.push_str(&format!("    registry.register(Box::new({name}::PluginInstance));\n"));
                }
            }
        }
    }

    let out_dir = std::env::var("OUT_DIR").unwrap();
    let manifest = format!(
        "{mod_decls}\n\
        pub fn register_all(registry: &mut crate::plugin::PluginRegistry) {{\n\
        {reg_calls}\
        }}\n"
    );

    fs::write(Path::new(&out_dir).join("plugins_manifest.rs"), manifest).unwrap();
}
