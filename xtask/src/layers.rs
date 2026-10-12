//! Dependency layer check. A crate may depend only on crates of a LOWER layer.
//! A new crate must be registered in `LAYERS` (otherwise the check fails).

use std::fs;
use std::path::Path;

/// (package name, layer). Applications (`dirustat`, `dirustat-cli`) sit above all layers: layer 99.
pub const LAYERS: &[(&str, u8)] = &[
    ("dirustat-model", 0),
    ("dirustat-treemap", 0),
    ("dirustat-ntfs", 0),
    ("dirustat-scan", 1),
    ("dirustat-engine", 2),
    ("dirustat-control", 3),
    ("dirustat-ui", 4),
    ("dirustat", 99),
    ("dirustat-cli", 99),
];

/// Dependencies forbidden below L4 and in the headless CLI (the GUI stack).
const GUI_DEPS: &[&str] = &["egui", "eframe", "winit", "rfd", "wgpu", "egui_extras"];

fn layer_of(name: &str) -> Option<u8> {
    LAYERS.iter().find(|(n, _)| *n == name).map(|(_, l)| *l)
}

/// Dependency names from `[dependencies]`, `[dev-dependencies]` and `[build-dependencies]` via a simple line parser.
fn dependency_names(manifest: &str) -> Vec<String> {
    let mut in_deps = false;
    let mut names = Vec::new();
    for line in manifest.lines().map(str::trim) {
        if line.starts_with('[') {
            in_deps = line == "[dependencies]" || line == "[dev-dependencies]" || line == "[build-dependencies]";
            continue;
        }
        if let (true, false, false, Some(key)) =
            (in_deps, line.is_empty(), line.starts_with('#'), line.split(['=', '.']).next())
        {
            names.push(key.trim().to_owned());
        }
    }
    names
}

fn package_name(manifest: &str) -> Option<String> {
    manifest.lines().map(str::trim).find_map(|l| l.strip_prefix("name = ")).map(|v| v.trim_matches('"').to_owned())
}

fn manifests(root: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for group in ["crates", "apps"] {
        let Ok(entries) = fs::read_dir(root.join(group)) else { continue };
        for entry in entries.flatten() {
            let path = entry.path().join("Cargo.toml");
            if path.is_file() {
                out.push(path);
            }
        }
    }
    out
}

/// Returns `true` when there are no violations.
pub fn check(root: &Path) -> bool {
    let mut ok = true;
    for path in manifests(root) {
        let Ok(text) = fs::read_to_string(&path) else {
            eprintln!("layers: cannot read {}", path.display());
            ok = false;
            continue;
        };
        let Some(name) = package_name(&text) else { continue };
        let Some(layer) = layer_of(&name) else {
            eprintln!("layers: crate `{name}` is not registered in xtask/src/layers.rs");
            ok = false;
            continue;
        };
        for dep in dependency_names(&text) {
            if layer < 4 && GUI_DEPS.contains(&dep.as_str())
                || name == "dirustat-cli" && GUI_DEPS.contains(&dep.as_str())
            {
                eprintln!("layers: `{name}` must not depend on GUI crate `{dep}`");
                ok = false;
            }
            if let Some(dep_layer) = layer_of(&dep) {
                let allowed = layer == 99 || dep_layer < layer;
                if !allowed || (name == "dirustat-cli" && dep == "dirustat-ui") {
                    eprintln!("layers: `{name}` (L{layer}) must not depend on `{dep}` (L{dep_layer})");
                    ok = false;
                }
            }
        }
    }
    if ok {
        println!("layers: ok");
    }
    ok
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_dependency_names() {
        let m = "[package]\nname = \"x\"\n\n[dependencies]\nfoo.workspace = true\nbar = { path = \"..\" }\n[lints]\nworkspace = true\n";
        assert_eq!(dependency_names(m), vec!["foo", "bar"]);
        assert_eq!(package_name(m).as_deref(), Some("x"));
    }
}
