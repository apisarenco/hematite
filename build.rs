use std::fs;
use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=src/plugins");

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
