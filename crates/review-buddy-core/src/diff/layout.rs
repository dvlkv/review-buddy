// TODO: port DiffLayout.swift (stub keeps the crate compiling).
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum DiffViewMode {
    #[default]
    Unified,
    Split,
}

/// Heuristics matching GitHub's linguist "generated" defaults for common files.
pub struct GeneratedFileDetector;

impl GeneratedFileDetector {
    const EXACT_NAMES: [&'static str; 12] = [
        "package-lock.json", "yarn.lock", "pnpm-lock.yaml", "Cargo.lock", "Gemfile.lock",
        "Podfile.lock", "composer.lock", "poetry.lock", "go.sum", "Package.resolved", "bun.lockb", "uv.lock",
    ];
    const SUFFIXES: [&'static str; 9] = [
        ".min.js", ".min.css", ".pb.go", "_pb2.py", ".pbxproj", ".snap", ".generated.swift", ".g.dart", ".map",
    ];

    pub fn is_generated(path: &str) -> bool {
        let name = path.rsplit('/').next().unwrap_or(path);
        if Self::EXACT_NAMES.contains(&name) {
            return true;
        }
        if Self::SUFFIXES.iter().any(|s| name.ends_with(s)) {
            return true;
        }
        path.contains("/__generated__/")
            || path.starts_with("dist/")
            || path.contains("/dist/")
            || path.starts_with("vendor/")
            || path.contains("/node_modules/")
    }
}
