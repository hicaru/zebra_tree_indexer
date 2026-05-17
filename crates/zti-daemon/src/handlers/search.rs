use zti_protocol::request::SearchReq;
use zti_protocol::response::{Response, SearchHit, SearchResults};
use zti_rerank::TurboReranker;

use crate::handlers::with_project;
use crate::state::DaemonState;

pub async fn handle(req: &SearchReq, state: &DaemonState) -> Response {
    let query = req.query.clone();
    let limit = req.limit;
    let languages = req.languages.clone();
    let path_glob = req.path_glob.clone();

    let result = with_project(state, &req.project_root, |project| async move {
        let dim = state.engine.dim();
        let chunks_table = project.db.chunks_table(dim).await?;
        let query_emb = state.engine.embed_query_async(&query).await?;

        let k = limit.saturating_mul(3);
        let candidates = chunks_table
            .knn(&query_emb, k, languages.as_deref(), path_glob.as_deref())
            .await?;

        let reranker = TurboReranker::new(dim)?;
        let rerank_input: Vec<(&[u8], f32)> = candidates
            .iter()
            .map(|c| (c.turbo_code.as_slice(), c.score))
            .collect();
        let ranked = reranker.rerank(&rerank_input, &query_emb);

        let hits: Vec<SearchHit> = ranked
            .into_iter()
            .take(limit)
            .filter_map(|(idx, score)| {
                candidates.get(idx).map(|c| SearchHit {
                    chunk_id: c.chunk_id.clone(),
                    file_path: c.file_path.clone(),
                    symbol_qualified: c.symbol_qualified.clone(),
                    start_line: c.start_line,
                    end_line: c.end_line,
                    content: c.content.clone(),
                    score,
                })
            })
            .collect();
        let total = hits.len();

        Ok(SearchResults { hits, total })
    })
    .await;

    Response::Search(result)
}
