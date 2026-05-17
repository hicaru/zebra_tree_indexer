use std::collections::HashMap;
use std::path::Path;

use anyhow::Result;
use ignore::WalkBuilder;

use zti_tree_sitter::{Language, detect_from_path, frontend_for};

use crate::model::{FileEntry, ProjectIndex};

const SKIP_DIRS: &[&str] = &[".git", "node_modules", "target", "build", "dist", ".cache"];

/// Collect the union of every supported language's `extra_skip_dirs` —
/// the standalone walker has no way to know which languages are present
/// before traversing, so we filter against the superset.
fn all_lang_skip_dirs() -> Vec<&'static str> {
    use zti_tree_sitter::Language;
    let mut out: Vec<&'static str> = Vec::new();
    for lang in [Language::Rust, Language::Ts, Language::Tsx, Language::Dart, Language::Solidity] {
        let cfg = zti_tree_sitter::frontend_for(lang).config();
        for &d in cfg.extra_skip_dirs {
            if !out.contains(&d) {
                out.push(d);
            }
        }
    }
    out
}

/// One source file the parser will index. Caller owns the path string (it is
/// moved into the resulting `FileEntry.path`); content is borrowed for the
/// duration of the parse.
pub struct SourceFile<'a> {
    pub full_path: String,
    pub content: &'a str,
    pub language: Language,
}

/// Build a `ProjectIndex` from already-loaded sources. This is the hot path
/// used by `zti-pipeline::indexer::index_project` — the indexer has already
/// walked the filesystem and read every file, so we must not walk a second
/// time.
pub fn build_index_from_sources<'a, I>(root: String, sources: I) -> ProjectIndex
where
    I: IntoIterator<Item = SourceFile<'a>>,
{
    let mut files: Vec<FileEntry> = Vec::new();
    let mut all_symbols: Vec<zti_ts_core::types::Symbol> = Vec::new();
    let mut all_edges: Vec<zti_ts_core::types::Edge> = Vec::new();

    for src in sources {
        let SourceFile { full_path, content, language } = src;
        let file_idx = files.len() as u16;
        let frontend = frontend_for(language);
        let id_offset = all_symbols.len() as u32;

        match frontend.parse(content, file_idx, id_offset) {
            Ok((symbols, edges, imports)) => {
                files.push(FileEntry {
                    path: full_path,
                    language,
                    imports,
                });
                all_symbols.extend(symbols);
                all_edges.extend(edges);
            }
            Err(e) => {
                tracing::warn!("Failed to parse {}: {}", full_path, e);
            }
        }
    }

    let qualified_map = build_qualified_map(&all_symbols, &files);
    resolve_edges(&mut all_edges, &files, &qualified_map, &all_symbols);

    let reverse_edges = build_reverse_edges(&all_edges);
    let forward_edges = build_forward_edges(&all_edges);

    ProjectIndex {
        symbols: all_symbols,
        edges: all_edges,
        files,
        qualified_map,
        reverse_edges,
        forward_edges,
        root,
    }
}

/// Standalone walker entry point — used by `zebra-dsl` and other callers that
/// don't have pre-walked sources. The pipeline must not use this path
/// (it would walk twice); use `build_index_from_sources` instead.
pub fn build_index(root: &str) -> Result<ProjectIndex> {
    let root_path = Path::new(root).canonicalize()?;

    // (full_path, content, language)
    let mut loaded: Vec<(String, String, Language)> = Vec::new();

    // Foundry layout: only skip the language's extra dirs if the marker file
    // is present. For non-Foundry repos `lib/` (rust crate dir!) must NOT be
    // dropped, so we gate the extra-skip list on `foundry.toml`.
    let is_forge = root_path.join("foundry.toml").exists();
    let lang_skip_dirs: Vec<&'static str> = if is_forge {
        all_lang_skip_dirs()
    } else {
        Vec::new()
    };
    let walker = WalkBuilder::new(&root_path)
        .hidden(false)
        .git_ignore(true)
        .filter_entry(move |entry| {
            if entry.file_type().is_some_and(|ft| ft.is_dir()) {
                let name = entry.file_name().to_string_lossy();
                if SKIP_DIRS.contains(&name.as_ref()) {
                    return false;
                }
                if lang_skip_dirs.contains(&name.as_ref()) {
                    return false;
                }
            }
            true
        })
        .build();

    for entry in walker {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let lang = match detect_from_path(path) {
            Some(l) => l,
            None => continue,
        };
        let path_str = path.to_string_lossy().to_string();
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        loaded.push((path_str, content, lang));
    }

    let sources = loaded
        .iter()
        .map(|(p, c, l)| SourceFile {
            full_path: p.clone(),
            content: c.as_str(),
            language: *l,
        });

    Ok(build_index_from_sources(
        root_path.to_string_lossy().to_string(),
        sources,
    ))
}

fn build_qualified_map(symbols: &[zti_ts_core::types::Symbol], files: &[FileEntry]) -> HashMap<String, u32> {
    let mut map = HashMap::new();

    let mut name_counts: HashMap<&str, usize> = HashMap::new();
    for sym in symbols {
        *name_counts.entry(sym.name.as_str()).or_insert(0) += 1;
    }

    for sym in symbols {
        let is_unique = name_counts.get(sym.name.as_str()) == Some(&1);
        if sym.qualified != sym.name || is_unique {
            map.entry(sym.qualified.clone()).or_insert(sym.id);
        }

        if let Some(file) = files.get(sym.file_idx as usize) {
            let short_path = file
                .path
                .rsplit('/')
                .next()
                .unwrap_or(&file.path)
                .trim_end_matches(".rs")
                .trim_end_matches(".ts")
                .trim_end_matches(".tsx")
                .trim_end_matches(".dart")
                .to_string();
            if short_path != sym.name {
                let file_qualified = format!("{}::{}", short_path, sym.name);
                map.entry(file_qualified).or_insert(sym.id);
            }
        }

        if is_unique {
            map.entry(sym.name.clone()).or_insert(sym.id);
        }
    }
    map
}

fn resolve_edges(
    edges: &mut [zti_ts_core::types::Edge],
    files: &[FileEntry],
    qualified_map: &HashMap<String, u32>,
    symbols: &[zti_ts_core::types::Symbol],
) {
    for edge in edges.iter_mut() {
        if let zti_ts_core::types::Target::Unresolved(name) = &edge.to {
            let name = name.clone();

            let resolved = if let Some(&id) = qualified_map.get(&name) {
                Some(id)
            } else if let Some(id) =
                resolve_via_imports(&name, edge.from, files, symbols, qualified_map)
            {
                Some(id)
            } else {
                resolve_in_same_file(&name, edge.from, symbols)
            };

            edge.to = match resolved {
                Some(id) => zti_ts_core::types::Target::Resolved(id),
                None => zti_ts_core::types::Target::External(format!("*{}", name)),
            };
        }
    }
}

fn resolve_via_imports(
    name: &str,
    from_id: u32,
    files: &[FileEntry],
    symbols: &[zti_ts_core::types::Symbol],
    qualified_map: &HashMap<String, u32>,
) -> Option<u32> {
    let from_sym = symbols.get(from_id as usize)?;
    let file = files.get(from_sym.file_idx as usize)?;

    if let Some(qualified_path) = file.imports.get(name) {
        if let Some(&id) = qualified_map.get(qualified_path) {
            return Some(id);
        }
        let qualified = format!("{}::{}", qualified_path, name);
        return qualified_map.get(&qualified).copied();
    }

    None
}

fn resolve_in_same_file(name: &str, from_id: u32, symbols: &[zti_ts_core::types::Symbol]) -> Option<u32> {
    let from_sym = symbols.get(from_id as usize)?;
    symbols
        .iter()
        .find(|s| s.file_idx == from_sym.file_idx && s.name == name)
        .map(|s| s.id)
}

fn build_reverse_edges(edges: &[zti_ts_core::types::Edge]) -> HashMap<u32, Vec<zti_ts_core::types::Edge>> {
    let mut reverse: HashMap<u32, Vec<zti_ts_core::types::Edge>> = HashMap::new();
    for edge in edges {
        if let zti_ts_core::types::Target::Resolved(target_id) = edge.to {
            let mut reverse_edge = edge.clone();
            reverse_edge.from = target_id;
            reverse_edge.to = zti_ts_core::types::Target::Resolved(edge.from);
            reverse.entry(target_id).or_default().push(reverse_edge);
        }
    }
    reverse
}

fn build_forward_edges(edges: &[zti_ts_core::types::Edge]) -> HashMap<u32, Vec<zti_ts_core::types::Edge>> {
    let mut forward: HashMap<u32, Vec<zti_ts_core::types::Edge>> = HashMap::new();
    for edge in edges {
        forward.entry(edge.from).or_default().push(edge.clone());
    }
    forward
}
