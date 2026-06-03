use std::collections::HashMap;
use std::path::Path;

use ignore::WalkBuilder;

use zti_store::FileRow;
use zti_tree_sitter::{Language, detect_from_path};

/// Project manifests we already render as `@ <path>` blocks in the chunk
/// preamble — skip them in the file walker so we don't re-embed the same
/// content as a text chunk.
const MANIFEST_NAMES: &[&str] = &["Cargo.toml", "pubspec.yaml", "package.json", "foundry.toml"];

fn is_lock_file(name: &str) -> bool {
    name.ends_with(".lock") || name.contains("-lock.")
}

/// Match the LICENSE family by stem (case-insensitive): `LICENSE`,
/// `LICENSE.md`, `LICENSE.txt`, `LICENSE-MIT`, `LICENSE-APACHE-2.0`, …
/// These are boilerplate; embedding them dilutes search results.
fn is_license_file(name: &str) -> bool {
    let stem = name.split(['.', '-']).next().unwrap_or(name);
    stem.eq_ignore_ascii_case("LICENSE")
}

const NON_CODE_ASSET_EXTS: &[&str] = &[".svg", ".ico", ".woff", ".woff2", ".ttf", ".eot", ".otf"];

fn is_non_code_asset(name: &str) -> bool {
    NON_CODE_ASSET_EXTS.iter().any(|ext| name.ends_with(ext))
}

const PIPELINE_SKIP_DIRS: &[&str] = &[
    "node_modules",
    "vendor",
    "third_party",
    "Pods",
    ".pub-cache",
    ".dart_tool",
    "target",
    "DerivedData",
    ".gradle",
    "__pycache__",
    ".mypy_cache",
    ".pytest_cache",
    ".ruff_cache",
    "venv",
    ".venv",
    "env",
    "virtualenv",
    ".terraform",
    "_build",
    "deps",
];

fn is_generated_file(name: &str) -> bool {
    if name.starts_with("frb_generated") {
        return true;
    }
    if name.ends_with(".freezed.dart") || name.ends_with(".g.dart") {
        return true;
    }
    if name.ends_with(".pb.dart")
        || name.ends_with(".pbjson.dart")
        || name.ends_with(".pbserver.dart")
        || name.ends_with(".grpc.dart")
    {
        return true;
    }
    if name.ends_with(".gen.dart")
        || name.ends_with(".gen.ts")
        || name.ends_with(".generated.dart")
        || name.ends_with(".generated.ts")
    {
        return true;
    }
    if name.ends_with(".min.js") || name.ends_with(".min.css") {
        return true;
    }
    if name.ends_with(".arb") || name.ends_with(".mo") {
        return true;
    }
    false
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    Code(Language),
    /// Tab-separated values. Chunked one row at a time so a dense database
    /// dump becomes one record per row, not thousands of byte-sized passages.
    Tsv,
    /// Pipe-separated values. Same row-aware chunking as `Tsv`.
    Psv,
    /// Any file we don't parse with tree-sitter but can read as UTF-8 text
    /// (READMEs, design docs, plain text, YAML, JSON, etc.). One chunk per
    /// file.
    Text,
}

impl SourceKind {
    pub fn label(&self) -> &'static str {
        match self {
            SourceKind::Code(lang) => lang.as_str(),
            SourceKind::Tsv => "tsv",
            SourceKind::Psv => "psv",
            SourceKind::Text => "text",
        }
    }
}

/// Classify a file path into a source kind by extension alone. Pure — does no
/// I/O — so the walker and tests share one source of truth.
fn classify_kind(path: &Path) -> SourceKind {
    match detect_from_path(path) {
        Some(l) => SourceKind::Code(l),
        None => match path.extension().and_then(|e| e.to_str()) {
            Some("tsv") => SourceKind::Tsv,
            Some("psv") => SourceKind::Psv,
            _ => SourceKind::Text,
        },
    }
}

#[derive(Debug, Clone)]
pub struct FileSnapshot {
    pub rel_path: String,
    pub mtime_ns: u128,
    pub blake3: [u8; 32],
    pub size_bytes: u64,
    pub contents: String,
    pub kind: SourceKind,
}

pub struct Changes {
    pub added: Vec<String>,
    pub modified: Vec<String>,
    pub removed: Vec<String>,
    pub unchanged: Vec<String>,
}

fn is_platform_scaffolding(rel_path: &str, has_lib_or_src: bool) -> bool {
    if !has_lib_or_src {
        return false;
    }
    rel_path.starts_with("ios/")
        || rel_path.starts_with("android/")
        || rel_path.starts_with("macos/")
        || rel_path.starts_with("linux/")
        || rel_path.starts_with("windows/")
        || rel_path.starts_with("web/")
        || rel_path.starts_with("rust_builder/")
        || rel_path.starts_with("fastlane/")
        || rel_path.starts_with("test/")
        || rel_path.starts_with("tests/")
        || rel_path.starts_with("integration_test/")
        || rel_path.starts_with("__tests__/")
        || rel_path.starts_with("spec/")
        || rel_path.starts_with("e2e/")
}

pub fn walk_source_files(root: &Path) -> HashMap<String, FileSnapshot> {
    let mut map = HashMap::new();

    let walker = WalkBuilder::new(root)
        .hidden(true)
        .git_ignore(true)
        .filter_entry(|entry| {
            if entry.file_type().is_some_and(|ft| ft.is_dir()) {
                let name = entry.file_name().to_string_lossy();
                !PIPELINE_SKIP_DIRS.contains(&name.as_ref())
            } else {
                true
            }
        })
        .build();

    let has_lib_or_src = root.join("lib").exists() || root.join("src").exists();

    for entry in walker {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if file_name.starts_with('.')
            || MANIFEST_NAMES.contains(&file_name)
            || is_lock_file(file_name)
            || is_license_file(file_name)
            || is_non_code_asset(file_name)
            || is_generated_file(file_name)
        {
            continue;
        }

        // Tree-sitter language if recognised, tabular for `.tsv`, otherwise
        // text. Binary files are filtered naturally by `read_to_string`
        // returning Err on invalid UTF-8.
        let kind = classify_kind(path);

        let rel = path
            .strip_prefix(root)
            .unwrap_or(path)
            .display()
            .to_string();

        if is_platform_scaffolding(&rel, has_lib_or_src) {
            continue;
        }

        let contents = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(_) => continue,
        };

        let metadata = match std::fs::metadata(path) {
            Ok(m) => m,
            Err(_) => continue,
        };
        let mtime_ns = metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos())
            .unwrap_or(0);

        let blake3: [u8; 32] = blake3::hash(contents.as_bytes()).into();

        let rel_path = rel.clone();
        map.insert(
            rel,
            FileSnapshot {
                rel_path,
                mtime_ns,
                blake3,
                size_bytes: metadata.len(),
                contents,
                kind,
            },
        );
    }

    map
}

pub fn detect_changes(current: &HashMap<String, FileSnapshot>, previous: &[FileRow]) -> Changes {
    let mut prev_map: HashMap<&str, &[u8]> = HashMap::with_capacity(previous.len());
    for row in previous {
        prev_map.insert(&row.file_path, &row.blake3);
    }

    let mut added = Vec::with_capacity(current.len());
    let mut modified = Vec::with_capacity(current.len());
    let mut unchanged = Vec::with_capacity(current.len());

    for (rel, snap) in current {
        match prev_map.get(rel.as_str()) {
            Some(&prev_hash) => {
                if snap.blake3.as_slice() == prev_hash {
                    unchanged.push(rel.clone());
                } else {
                    modified.push(rel.clone());
                }
            }
            None => {
                added.push(rel.clone());
            }
        }
    }

    let current_set: HashMap<&str, ()> = current.keys().map(|k| (k.as_str(), ())).collect();
    let removed = previous
        .iter()
        .filter(|row| !current_set.contains_key(row.file_path.as_str()))
        .map(|row| row.file_path.clone())
        .collect();

    Changes {
        added,
        modified,
        removed,
        unchanged,
    }
}

#[cfg(test)]
mod tests {
    use super::{SourceKind, classify_kind};
    use std::path::Path;
    use zti_tree_sitter::Language;

    #[test]
    fn tsv_is_tabular_not_text() {
        assert_eq!(classify_kind(Path::new("db/findings.tsv")), SourceKind::Tsv);
    }

    #[test]
    fn psv_is_its_own_kind() {
        assert_eq!(classify_kind(Path::new("db/findings.psv")), SourceKind::Psv);
    }

    #[test]
    fn code_extensions_unchanged() {
        assert_eq!(
            classify_kind(Path::new("src/main.rs")),
            SourceKind::Code(Language::Rust),
        );
        assert_eq!(
            classify_kind(Path::new("app/widget.dart")),
            SourceKind::Code(Language::Dart),
        );
    }

    #[test]
    fn docs_remain_plain_text() {
        assert_eq!(classify_kind(Path::new("README.md")), SourceKind::Text);
        assert_eq!(classify_kind(Path::new("data.csv")), SourceKind::Text);
    }
}
