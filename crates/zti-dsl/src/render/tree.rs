use std::collections::HashSet;
use std::fmt::Write as _;

use zti_ts_core::types::{Edge, EdgeKind, Target};

use crate::model::ProjectIndex;

pub struct AsciiTreeRenderer<'a> {
    index: &'a ProjectIndex,
}

impl<'a> AsciiTreeRenderer<'a> {
    pub fn new(index: &'a ProjectIndex) -> Self {
        Self { index }
    }

    pub fn render_callers(&self, id: u32, max_depth: usize) -> String {
        let mut out = String::new();
        let sym = match self.index.symbols.get(id as usize) {
            Some(s) => s,
            None => return format!("Symbol {} not found\n", id),
        };
        let _ = writeln!(out, "{}#{} {} (callers)", sym.kind.short(), id, sym.qualified);
        let mut prefix = String::new();
        let mut visited = HashSet::new();
        self.recurse(
            id,
            max_depth,
            0,
            &mut out,
            &mut prefix,
            &mut visited,
            Direction::Callers,
            false,
            true,
        );
        out
    }

    pub fn render_callees(&self, id: u32, max_depth: usize, local_only: bool) -> String {
        self.render_callees_with_ids(id, max_depth, local_only, true)
    }

    pub fn render_callees_with_ids(&self, id: u32, max_depth: usize, local_only: bool, show_ids: bool) -> String {
        let mut out = String::new();
        let sym = match self.index.symbols.get(id as usize) {
            Some(s) => s,
            None => return format!("Symbol {} not found\n", id),
        };
        if show_ids {
            let _ = writeln!(out, "{}#{} {} (callees)", sym.kind.short(), id, sym.qualified);
        } else {
            let _ = writeln!(out, "{} {} (callees)", sym.kind.short(), sym.qualified);
        }
        let mut prefix = String::new();
        let mut visited = HashSet::new();
        self.recurse(
            id,
            max_depth,
            0,
            &mut out,
            &mut prefix,
            &mut visited,
            Direction::Callees,
            local_only,
            show_ids,
        );
        out
    }

    /// One recursive descent; direction selects the edge map and target field.
    /// `prefix` is an accumulator passed down — each level pushes its own
    /// segment and truncates on the way back, so we allocate zero strings per
    /// visited node.
    #[allow(clippy::too_many_arguments)]
    fn recurse(
        &self,
        id: u32,
        max_depth: usize,
        depth: usize,
        out: &mut String,
        prefix: &mut String,
        visited: &mut HashSet<u32>,
        direction: Direction,
        local_only: bool,
        show_ids: bool,
    ) {
        if depth >= max_depth || !visited.insert(id) {
            return;
        }

        let edges_for_id: &[Edge] = match direction {
            Direction::Callers => self.index.reverse_edges.get(&id).map(Vec::as_slice).unwrap_or(&[]),
            Direction::Callees => self.index.forward_edges.get(&id).map(Vec::as_slice).unwrap_or(&[]),
        };

        let filtered: Vec<&Edge> = edges_for_id
            .iter()
            .filter(|e| e.kind == EdgeKind::Call)
            .filter(|e| {
                if local_only {
                    matches!(e.to, Target::Resolved(_))
                } else {
                    true
                }
            })
            .collect();

        let total = filtered.len();
        if total == 0 {
            return;
        }

        for (i, edge) in filtered.iter().enumerate() {
            let is_last = i + 1 == total;
            let branch = if is_last { "└── " } else { "├── " };
            let child_segment = if is_last { "    " } else { "│   " };

            out.push_str(prefix);
            out.push_str(branch);

            match direction {
                Direction::Callers => {
                    if let Target::Resolved(from_id) = edge.to {
                        if let Some(sym) = self.index.symbols.get(from_id as usize) {
                            if show_ids {
                                let _ = writeln!(out, "{}#{} {}", sym.kind.short(), from_id, sym.qualified);
                            } else {
                                let _ = writeln!(out, "{} {}", sym.kind.short(), sym.qualified);
                            }
                            let saved = prefix.len();
                            prefix.push_str(child_segment);
                            self.recurse(
                                from_id,
                                max_depth,
                                depth + 1,
                                out,
                                prefix,
                                visited,
                                direction,
                                local_only,
                                show_ids,
                            );
                            prefix.truncate(saved);
                        } else {
                            out.push('\n');
                        }
                    } else {
                        out.push('\n');
                    }
                }
                Direction::Callees => match &edge.to {
                    Target::Resolved(to_id) => {
                        if let Some(sym) = self.index.symbols.get(*to_id as usize) {
                            if show_ids {
                                let _ = writeln!(out, "{}#{} {}", sym.kind.short(), to_id, sym.qualified);
                            } else {
                                let _ = writeln!(out, "{} {}", sym.kind.short(), sym.qualified);
                            }
                            let saved = prefix.len();
                            prefix.push_str(child_segment);
                            self.recurse(
                                *to_id,
                                max_depth,
                                depth + 1,
                                out,
                                prefix,
                                visited,
                                direction,
                                local_only,
                                show_ids,
                            );
                            prefix.truncate(saved);
                        } else {
                            out.push('\n');
                        }
                    }
                    Target::External(name) => {
                        let _ = writeln!(out, "*{}", name);
                    }
                    Target::Unresolved(name) => {
                        let _ = writeln!(out, "?{}", name);
                    }
                },
            }
        }
    }
}

#[derive(Clone, Copy)]
enum Direction {
    Callers,
    Callees,
}
