use std::borrow::Cow;
use std::collections::HashSet;
use std::fmt::Write as _;

use zti_ts_core::types::{Edge, EdgeKind, Kind, Target};

use crate::model::ProjectIndex;

pub const LEGEND_LINE: &str = "# k short = Kind   f=fn m=method s=struct e=enum C=class I=iface t=typealias c=const v=static .=field/variant E=event X=error M=mod";

pub struct InlineOpts {
    pub max_inline_targets: usize,
    pub max_doc_lines: usize,
    pub show_file_path: bool,
    pub show_line_range: bool,
}

impl InlineOpts {
    pub fn for_embedding() -> Self {
        Self {
            max_inline_targets: 12,
            max_doc_lines: 2,
            show_file_path: true,
            show_line_range: true,
        }
    }

    pub fn compact() -> Self {
        Self {
            max_inline_targets: 4,
            max_doc_lines: 1,
            show_file_path: false,
            show_line_range: false,
        }
    }
}

pub fn render_symbol_inline(
    index: &ProjectIndex,
    id: u32,
    opts: &InlineOpts,
    out: &mut String,
) {
    let sym = match index.symbols.get(id as usize) {
        Some(s) => s,
        None => return,
    };
    let file = index.files.get(sym.file_idx as usize);

    let short = sym.kind.short();
    out.push_str(short);
    out.push('#');
    let _ = write!(out, "{}", sym.id);
    out.push(' ');
    out.push_str(&sym.qualified);

    if opts.show_file_path
        && let Some(f) = file
    {
        let rel = f.path.strip_prefix(&index.root).unwrap_or(&f.path);
        let rel = rel.trim_start_matches('/');
        out.push(' ');
        out.push_str(rel);
    }

    if opts.show_line_range {
        let _ = write!(out, " :{}-{}", sym.line, sym.end_line);
    }

    if let Some(ref doc) = sym.doc {
        let mut joined = String::new();
        let mut seen = 0usize;
        for line in doc.lines().take(opts.max_doc_lines) {
            if seen > 0 {
                joined.push(' ');
            }
            joined.push_str(line);
            seen += 1;
        }
        if seen > 0 {
            out.push(' ');
            out.push('"');
            out.push_str(joined.trim());
            out.push('"');
        }
    }

    write_targets(
        out,
        " <- ",
        index
            .reverse_edges
            .get(&id)
            .into_iter()
            .flat_map(|v| v.iter())
            .filter(|e| e.kind == EdgeKind::Call),
        opts.max_inline_targets,
        |edge, out| {
            if let Some(ts) = index.symbols.get(edge.from as usize) {
                out.push_str(&ts.qualified);
            }
        },
    );

    write_targets(
        out,
        " -> ",
        index
            .forward_edges
            .get(&id)
            .into_iter()
            .flat_map(|v| v.iter())
            .filter(|e| e.kind == EdgeKind::Call),
        opts.max_inline_targets,
        |edge, out| {
            out.push_str(&format_target(&edge.to));
        },
    );
}

/// Inline-render up to `max` edge targets, followed by `...` if the iterator
/// would have produced more. Single-pass — no intermediate `Vec<&Edge>`.
fn write_targets<'a, I, F>(out: &mut String, prefix: &str, edges: I, max: usize, mut write_one: F)
where
    I: IntoIterator<Item = &'a Edge>,
    F: FnMut(&Edge, &mut String),
{
    let mut iter = edges.into_iter().peekable();
    if iter.peek().is_none() {
        return;
    }
    out.push_str(prefix);
    let mut overflow = false;
    for (i, edge) in iter.enumerate() {
        if i >= max {
            overflow = true;
            break;
        }
        if i > 0 {
            out.push(' ');
        }
        write_one(edge, out);
    }
    if overflow {
        out.push_str(" ...");
    }
}

pub fn format_target(target: &Target) -> Cow<'_, str> {
    match target {
        Target::Resolved(id) => Cow::Owned(format!("#{}", id)),
        Target::Unresolved(name) | Target::External(name) => Cow::Borrowed(name),
    }
}

pub struct DslRenderer<'a> {
    index: &'a ProjectIndex,
    max_tokens: usize,
}

impl<'a> DslRenderer<'a> {
    pub fn new(index: &'a ProjectIndex, max_tokens: usize) -> Self {
        Self { index, max_tokens }
    }

    pub fn render(
        &self,
        file_filter: Option<&[u16]>,
        kind_filter: Option<&[Kind]>,
    ) -> String {
        let mut out = String::new();
        let mut tokens_used = 0;

        let opts = InlineOpts::for_embedding();

        let filter_set: Option<HashSet<u16>> = file_filter.map(|f| f.iter().copied().collect());
        let kind_set: Option<HashSet<Kind>> = kind_filter.map(|k| k.iter().copied().collect());

        let mut last_file: Option<u16> = None;

        for sym in &self.index.symbols {
            if let Some(ref set) = filter_set
                && !set.contains(&sym.file_idx)
            {
                continue;
            }
            if let Some(ref set) = kind_set
                && !set.contains(&sym.kind)
            {
                continue;
            }

            if last_file != Some(sym.file_idx) {
                if let Some(file) = self.index.files.get(sym.file_idx as usize) {
                    let _ = writeln!(out, "FILES #{} {}", sym.file_idx, file.path);
                }
                last_file = Some(sym.file_idx);
            }

            let mut line = String::new();
            render_symbol_inline(self.index, sym.id, &opts, &mut line);
            out.push_str(&line);
            out.push('\n');

            tokens_used += line.len() / crate::render::CHARS_PER_TOKEN;
            if tokens_used >= self.max_tokens {
                out.push_str("... (truncated)\n");
                break;
            }
        }

        out
    }
}

pub fn render_files_only(index: &ProjectIndex, file_indices: &[u16]) -> String {
    let mut out = String::new();
    out.push_str("FILES (#N = file ID — appears as \"# N\" in project_map output)\n");
    for &idx in file_indices {
        if let Some(file) = index.files.get(idx as usize) {
            let _ = writeln!(out, "# {} {}", idx, file.path);
        }
    }
    out
}
