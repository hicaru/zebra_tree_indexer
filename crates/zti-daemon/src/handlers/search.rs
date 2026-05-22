use std::borrow::Cow;
use std::collections::HashMap;
use std::path::Path;

use zti_dsl::lang_search_legend;
use zti_protocol::request::SearchReq;
use zti_protocol::response::{Response, SearchHit, SearchResults};
use zti_rerank::TurboReranker;
use zti_store::chunks_table::ChunkHit;
use zti_tree_sitter::detect_from_path;

use crate::handlers::with_project;
use crate::state::DaemonState;

const APPENDIX_CAP: usize = 8;

pub async fn handle(req: &SearchReq, state: &DaemonState) -> Response {
    let result = with_project(state, &req.project_root, |project| async move {
        let pid =
            zti_common::ids::project_id(&std::path::Path::new(&req.project_root).canonicalize()?);

        let opts = zti_pipeline::search::SearchOpts {
            limit: req.limit,
            languages: req.languages.as_deref(),
            path_glob: req.path_glob.as_deref(),
        };
        let hits = if req.exhaustive {
            zti_pipeline::search::search_exhaustive(
                &req.query,
                &state.engine,
                &project.db,
                &pid,
                &opts,
            )
            .await?
        } else {
            let reranker = TurboReranker::new(state.engine.dim())?;
            zti_pipeline::search::search(
                &req.query,
                &state.engine,
                &project.db,
                &reranker,
                &state.ann,
                &pid,
                &opts,
            )
            .await?
        };

        let chunks_table = project.db.chunks_table(state.engine.dim()).await?;

        // Walk `hits` once to collect (a) sym_ids already in the top-N (so the
        // appendix dedupe HashSet is seeded) and (b) the appendix candidate
        // ids — both reads need only borrows. After this scan we are free to
        // consume `hits` by value and move every `ChunkHit` into `search_hits`
        // without cloning the heap-allocated String fields.
        let mut seen: std::collections::HashSet<u32> =
            std::collections::HashSet::with_capacity(hits.len() + APPENDIX_CAP);
        for h in &hits {
            seen.insert(h.chunk.sym_id);
        }
        let mut appendix_ids: Vec<u32> = Vec::with_capacity(APPENDIX_CAP);
        'outer: for h in &hits {
            for &sid in &h.chunk.appendix_sym_ids {
                if appendix_ids.len() >= APPENDIX_CAP {
                    break 'outer;
                }
                if seen.insert(sid) {
                    appendix_ids.push(sid);
                }
            }
        }

        let search_hits: Vec<SearchHit> = hits
            .into_iter()
            .map(|h| chunk_to_hit(h.chunk, h.score, &req.project_root))
            .collect();

        let appendix = if appendix_ids.is_empty() {
            Vec::with_capacity(0)
        } else {
            let rows = chunks_table.get_by_sym_ids(&appendix_ids).await?;
            // Move rows into a sym_id → ChunkHit map so the loop below can
            // pop owned ChunkHits via `.remove(sid)` — no .clone() on the
            // heap String fields.
            let mut by_sym: HashMap<u32, ChunkHit> = HashMap::with_capacity(rows.len());
            for r in rows {
                by_sym.insert(r.sym_id, r);
            }
            let mut out: Vec<SearchHit> = Vec::with_capacity(appendix_ids.len());
            for sid in &appendix_ids {
                if let Some(c) = by_sym.remove(sid) {
                    out.push(chunk_to_hit(c, 0.0, &req.project_root));
                }
            }
            out
        };

        let total = search_hits.len();
        let legend = build_legend(&search_hits, &appendix);

        Ok(SearchResults {
            hits: search_hits,
            appendix,
            legend,
            total,
        })
    })
    .await;

    Response::Search(result)
}

/// Emit one legend line per programming language that appears in the result
/// set. Single-language responses borrow a `&'static str` (zero allocation);
/// multi-language responses build one `String` joined by `\n`. Dedup keys are
/// pointer-equal `&'static str` legends, so `Ts`+`Tsx` collapse to one line.
fn build_legend(hits: &[SearchHit], appendix: &[SearchHit]) -> Cow<'static, str> {
    let mut legends: Vec<&'static str> = Vec::with_capacity(2);

    let mut push_for = |path: &str| {
        if let Some(lang) = detect_from_path(Path::new(path)) {
            let leg = lang_search_legend(lang);
            if !legends.iter().any(|l| std::ptr::eq(*l, leg)) {
                legends.push(leg);
            }
        }
    };

    for h in hits {
        push_for(&h.file_path);
    }
    for h in appendix {
        push_for(&h.file_path);
    }

    match legends.as_slice() {
        [] => Cow::Borrowed(""),
        [only] => Cow::Borrowed(*only),
        many => {
            let cap = many.iter().map(|l| l.len() + 1).sum::<usize>();
            let mut out = String::with_capacity(cap);
            for (i, line) in many.iter().enumerate() {
                if i > 0 {
                    out.push('\n');
                }
                out.push_str(line);
            }
            Cow::Owned(out)
        }
    }
}

/// Consume a `ChunkHit` and produce the wire-format `SearchHit`. All heap
/// fields (`chunk_id`, `symbol_qualified`, `symbol_kind`, `content`) are moved
/// — no `.clone()`. `file_path` is rewritten in place: the project-root
/// prefix and any leading slashes are removed via `String::drain` so the
/// existing heap allocation is reused.
fn chunk_to_hit(mut c: ChunkHit, score: f32, project_root: &str) -> SearchHit {
    if c.file_path.starts_with(project_root) {
        c.file_path.drain(..project_root.len());
    }
    let lead_slashes = c.file_path.bytes().take_while(|b| *b == b'/').count();
    if lead_slashes > 0 {
        c.file_path.drain(..lead_slashes);
    }
    SearchHit {
        chunk_id: c.chunk_id,
        file_path: c.file_path,
        symbol_qualified: c.symbol_qualified,
        symbol_kind: c.symbol_kind,
        sym_id: c.sym_id,
        start_line: c.start_line,
        end_line: c.end_line,
        content: c.content,
        score,
    }
}
