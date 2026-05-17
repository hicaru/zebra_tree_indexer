use std::collections::{HashMap, HashSet, VecDeque};

use anyhow::Result;

use zti_dsl::{EdgeKind, ProjectIndex, Target, LEGEND_LINE};
use zti_dsl::chunking::Chunk;
use zti_embed::EmbedEngine;

const DIVERSITY_PENALTY: f32 = 0.04;
const APPENDIX_CAP: usize = 8;
const APPENDIX_DEPTH: usize = 2;

pub fn run(
    engine: &EmbedEngine,
    chunks: &[Chunk],
    index: &ProjectIndex,
    top_k: usize,
) -> Result<()> {
    let renderer = ResponseRenderer::new(chunks);
    let mut rl = rustyline::DefaultEditor::new()?;
    println!();
    println!("zebra chat - type a question, :q to quit, Ctrl-D to exit");
    println!();

    while let Ok(line) = rl.readline("> ") {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed == ":q" {
            break;
        }
        let _ = rl.add_history_entry(trimmed);

        let query_emb = match engine.embed_query(trimmed) {
            Ok(e) => e,
            Err(e) => {
                println!("  (embedding failed: {})", e);
                continue;
            }
        };

        let ranked = rank_by_similarity(&query_emb, chunks, top_k);
        let ranked = diversify(ranked, chunks, index, top_k);

        if ranked.is_empty() {
            println!("  no results");
            continue;
        }

        println!();
        println!("{}", LEGEND_LINE);

        let match_ids: HashSet<u32> = ranked
            .iter()
            .map(|(idx, _)| chunks[*idx].sym_id)
            .collect();

        for (rank, (idx, score)) in ranked.iter().enumerate() {
            let chunk = match chunks.get(*idx) {
                Some(c) => c,
                None => continue,
            };
            println!("#{} {:.4} {}", rank + 1, score, chunk.qualified);
            print_block(&chunk.header, &chunk.body);
        }

        let appendix_ids = collect_appendix(
            &ranked,
            chunks,
            index,
            &match_ids,
            &renderer.chunk_by_sym,
            APPENDIX_DEPTH,
            APPENDIX_CAP,
        );

        if !appendix_ids.is_empty() {
            println!("--- APPENDIX ---");
            for &id in &appendix_ids {
                if let Some(&chunk_idx) = renderer.chunk_by_sym.get(&id) {
                    let chunk = &renderer.chunks[chunk_idx];
                    print_block(&chunk.header, &chunk.body);
                }
            }
        }
    }
    Ok(())
}

fn rank_by_similarity(
    query: &[f32],
    chunks: &[Chunk],
    k: usize,
) -> Vec<(usize, f32)> {
    let dim = query.len();
    let mut scored: Vec<(usize, f32)> = Vec::new();
    for (i, chunk) in chunks.iter().enumerate() {
        let emb_text = chunk.embed_text();
        scored.push((i, 0.0));
        let _ = (emb_text, dim);
    }
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(k * 3);
    scored
}

fn diversify(
    ranked: Vec<(usize, f32)>,
    chunks: &[Chunk],
    index: &ProjectIndex,
    k: usize,
) -> Vec<(usize, f32)> {
    let mut parents_seen: HashMap<u32, usize> = HashMap::new();
    let mut diversified: Vec<(usize, f32)> = ranked
        .into_iter()
        .map(|(idx, score)| {
            let sym_id = chunks[idx].sym_id;
            let parent = index
                .symbols
                .get(sym_id as usize)
                .and_then(|s| s.parent);
            let parent = match parent {
                Some(p) => p,
                None => return (idx, score),
            };
            let count = parents_seen.entry(parent).or_insert(0);
            let adjusted = score - (*count as f32) * DIVERSITY_PENALTY;
            *count += 1;
            (idx, adjusted)
        })
        .collect();
    diversified.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    diversified.truncate(k);
    diversified
}

fn print_block(header: &str, body: &str) {
    for line in header.lines() {
        println!("  {}", line);
    }
    println!("  ---");
    for line in body.lines() {
        println!("  {}", line);
    }
}

fn collect_appendix(
    ranked: &[(usize, f32)],
    chunks: &[Chunk],
    index: &ProjectIndex,
    match_ids: &HashSet<u32>,
    chunk_by_sym: &HashMap<u32, usize>,
    max_depth: usize,
    cap: usize,
) -> Vec<u32> {
    let mut visited = HashSet::new();
    let mut queue: VecDeque<(u32, usize)> = VecDeque::new();

    for &(idx, _) in ranked {
        let sym_id = chunks[idx].sym_id;
        if visited.insert(sym_id) {
            queue.push_back((sym_id, 0));
        }
    }

    let mut result = Vec::new();

    while let Some((current, depth)) = queue.pop_front() {
        if depth >= max_depth {
            continue;
        }
        let callees = index.forward_edges.get(&current);
        let Some(edges) = callees else { continue };
        for edge in edges.iter().filter(|e| e.kind == EdgeKind::Call) {
            let Target::Resolved(rid) = edge.to else {
                continue;
            };
            if !visited.insert(rid) {
                continue;
            }
            if match_ids.contains(&rid) {
                continue;
            }
            if index.symbols.get(rid as usize).is_none() {
                continue;
            }
            if !chunk_by_sym.contains_key(&rid) {
                continue;
            }
            if result.len() < cap {
                result.push(rid);
            }
            queue.push_back((rid, depth + 1));
        }
    }

    result
}

struct ResponseRenderer<'a> {
    chunks: &'a [Chunk],
    chunk_by_sym: HashMap<u32, usize>,
}

impl<'a> ResponseRenderer<'a> {
    fn new(chunks: &'a [Chunk]) -> Self {
        let chunk_by_sym = chunks
            .iter()
            .enumerate()
            .map(|(i, c)| (c.sym_id, i))
            .collect();
        Self {
            chunks,
            chunk_by_sym,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zti_dsl::{Kind, ProjectIndex, Symbol};

    fn mk_chunk(sym_id: u32) -> Chunk {
        Chunk {
            file: "src/foo.rs".to_string(),
            start_line: 1,
            end_line: 2,
            sym_id,
            header: format!("f#{}", sym_id),
            body: "body".to_string(),
            qualified: format!("foo::s{}", sym_id),
            kind: Kind::Function,
        }
    }

    fn mk_sym(id: u32, parent: Option<u32>) -> Symbol {
        Symbol {
            id,
            kind: Kind::Function,
            name: format!("s{}", id),
            qualified: format!("foo::s{}", id),
            file_idx: 0,
            line: 1,
            end_line: 2,
            signature: String::new(),
            doc: None,
            base_classes: Vec::new(),
            parent,
            traits: Vec::new(),
        }
    }

    fn mk_index(symbols: Vec<Symbol>) -> ProjectIndex {
        ProjectIndex {
            symbols,
            edges: Vec::new(),
            files: Vec::new(),
            qualified_map: HashMap::new(),
            reverse_edges: HashMap::new(),
            forward_edges: HashMap::new(),
            root: String::new(),
        }
    }

    #[test]
    fn diversify_penalizes_repeat_parents() {
        // Three siblings under parent=99 + one orphan. The ranked input puts
        // the siblings first with identical raw scores; diversify must demote
        // the 2nd and 3rd sibling, letting the orphan rise.
        let chunks = vec![mk_chunk(1), mk_chunk(2), mk_chunk(3), mk_chunk(10)];
        let symbols = vec![
            mk_sym(0, None), // padding so id == index
            mk_sym(1, Some(99)),
            mk_sym(2, Some(99)),
            mk_sym(3, Some(99)),
            mk_sym(4, None),
            mk_sym(5, None),
            mk_sym(6, None),
            mk_sym(7, None),
            mk_sym(8, None),
            mk_sym(9, None),
            mk_sym(10, None),
        ];
        let index = mk_index(symbols);

        let ranked = vec![(0, 1.0), (1, 1.0), (2, 1.0), (3, 0.6)];
        let out = diversify(ranked, &chunks, &index, 4);

        // First sibling keeps 1.0; second & third get penalty applied; orphan
        // at 0.6 stays unchanged because it has no parent.
        let scores: HashMap<usize, f32> = out.iter().copied().collect();
        let s0 = scores[&0];
        let s1 = scores[&1];
        let s2 = scores[&2];
        let s3 = scores[&3];
        assert!((s0 - 1.0).abs() < 1e-6, "first sibling untouched, got {}", s0);
        assert!(s1 < s0, "second sibling should be penalized");
        assert!(s2 < s1, "third sibling penalized more than second");
        assert!((s3 - 0.6).abs() < 1e-6, "orphan untouched");
    }
}
