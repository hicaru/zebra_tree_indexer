use std::collections::HashMap;

use zti_dsl::LEGEND_LINE;
use zti_protocol::request::SearchReq;
use zti_protocol::response::{Response, SearchHit, SearchResults};
use zti_rerank::TurboReranker;
use zti_store::chunks_table::ChunkHit;

use crate::handlers::with_project;
use crate::state::DaemonState;

const APPENDIX_CAP: usize = 8;

pub async fn handle(req: &SearchReq, state: &DaemonState) -> Response {
    let query = req.query.clone();
    let limit = req.limit;

    let result = with_project(state, &req.project_root, |project| async move {
        let pid = zti_common::ids::project_id(
            &std::path::Path::new(&req.project_root).canonicalize()?,
        );

        let opts = zti_pipeline::search::SearchOpts {
            limit,
            languages: req.languages.as_deref(),
            path_glob: req.path_glob.as_deref(),
        };
        let reranker = TurboReranker::new(state.engine.dim())?;
        let hits = zti_pipeline::search::search(
            &query,
            &state.engine,
            &project.db,
            &reranker,
            &state.ann,
            &pid,
            &opts,
        )
        .await?;

        let chunks_table = project.db.chunks_table(state.engine.dim()).await?;

        let search_hits: Vec<SearchHit> = hits
            .iter()
            .map(|h| chunk_to_hit(&h.chunk, h.score, &req.project_root))
            .collect();

        let mut seen: std::collections::HashSet<u32> =
            std::collections::HashSet::with_capacity(search_hits.len() + APPENDIX_CAP);
        for h in &search_hits {
            seen.insert(h.sym_id);
        }
        let mut appendix_ids: Vec<u32> = Vec::with_capacity(APPENDIX_CAP);
        for h in &hits {
            for &sid in &h.chunk.appendix_sym_ids {
                if appendix_ids.len() >= APPENDIX_CAP {
                    break;
                }
                if seen.insert(sid) {
                    appendix_ids.push(sid);
                }
            }
            if appendix_ids.len() >= APPENDIX_CAP {
                break;
            }
        }

        let appendix = if appendix_ids.is_empty() {
            Vec::new()
        } else {
            let rows = chunks_table.get_by_sym_ids(&appendix_ids).await?;
            let by_sym: HashMap<u32, &ChunkHit> =
                rows.iter().map(|r| (r.sym_id, r)).collect();
            let mut out: Vec<SearchHit> = Vec::with_capacity(appendix_ids.len());
            for sid in &appendix_ids {
                if let Some(c) = by_sym.get(sid) {
                    out.push(chunk_to_hit(c, 0.0, &req.project_root));
                }
            }
            out
        };

        let total = search_hits.len();

        Ok(SearchResults {
            hits: search_hits,
            appendix,
            legend: std::borrow::Cow::Borrowed(LEGEND_LINE),
            total,
        })
    })
    .await;

    Response::Search(result)
}

fn chunk_to_hit(c: &ChunkHit, score: f32, project_root: &str) -> SearchHit {
    let rel = c
        .file_path
        .strip_prefix(project_root)
        .unwrap_or(&c.file_path)
        .trim_start_matches('/');
    SearchHit {
        chunk_id: c.chunk_id.clone(),
        file_path: rel.to_string(),
        symbol_qualified: c.symbol_qualified.clone(),
        symbol_kind: c.symbol_kind.clone(),
        sym_id: c.sym_id,
        start_line: c.start_line,
        end_line: c.end_line,
        content: c.content.clone(),
        score,
    }
}
