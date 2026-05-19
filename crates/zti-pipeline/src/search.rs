use std::cmp::Ordering;
use std::collections::HashMap;

use anyhow::{anyhow, Result};

use zti_ann::{AnnCache, AnnHandle, SearchMethod, SearchParams};
use zti_embed::EmbedEngine;
use zti_rerank::TurboReranker;
use zti_store::chunks_table::ChunkHit;

const KNN_OVERFETCH_MULT: usize = 4;
const DIVERSITY_PENALTY: f32 = 0.04;

pub struct SearchOpts<'a> {
    pub limit: usize,
    pub languages: Option<&'a [String]>,
    pub path_glob: Option<&'a str>,
}

pub struct Hit {
    pub chunk: ChunkHit,
    pub score: f32,
}

pub async fn search(
    query: &str,
    engine: &EmbedEngine,
    db: &zti_store::Db,
    reranker: &TurboReranker,
    ann_cache: &AnnCache,
    pid: &[u8; 32],
    opts: &SearchOpts<'_>,
) -> Result<Vec<Hit>> {
    let projects = db.projects_table().await?;
    let project = projects.get(pid).await?
        .ok_or_else(|| anyhow!("project not indexed"))?;

    let previous: Option<SearchParams> = project.search_params.as_deref()
        .and_then(|s| serde_json::from_str(s).ok());

    let params: SearchParams = match previous {
        Some(p) => p,
        None => zti_ann::choose_method(
            project.total_chunks as usize,
            engine.dim(),
            &zti_hw::probe(),
            None,
        ),
    };

    let query_emb = engine.embed_query_async(query).await?;
    let chunks_table = db.chunks_table(engine.dim()).await?;
    let raw_k = opts.limit.saturating_mul(KNN_OVERFETCH_MULT);

    let mut candidates: Vec<ChunkHit> = match params.method {
        SearchMethod::IvfHnswSq | SearchMethod::Flat => {
            chunks_table
                .knn(&query_emb, raw_k, &params, opts.languages, opts.path_glob)
                .await?
        }
        SearchMethod::HnswRs => {
            let graph: AnnHandle = ann_cache
                .get_or_build(*pid, || rebuild(&chunks_table, engine.dim(), &params))
                .await
                .map_err(|e: anyhow::Error| e)?;

            let mut topn: Vec<([u8; 16], f32)> = Vec::with_capacity(raw_k);
            graph.search(&query_emb, raw_k, opts.limit * 2, &mut topn);

            let score_by_id: std::collections::HashMap<[u8; 16], f32> = topn
                .iter()
                .map(|(id, score)| (*id, *score))
                .collect();

            let ids: Vec<[u8; 16]> = topn.iter().map(|(id, _)| *id).collect();
            let mut fetched = chunks_table
                .fetch_by_chunk_ids(&ids, opts.languages, opts.path_glob)
                .await?;

            for hit in &mut fetched {
                let mut key = [0u8; 16];
                key.copy_from_slice(&hit.chunk_id[..16]);
                if let Some(s) = score_by_id.get(&key) {
                    hit.score = *s;
                }
            }
            fetched
        }
    };

    let rerank_input: Vec<(&[u8], f32)> = candidates
        .iter()
        .map(|c| (c.turbo_code.as_slice(), c.score))
        .collect();
    let mut ranked = reranker.rerank(&rerank_input, &query_emb);

    diversify_by_parent_in_place(&mut ranked, &candidates, opts.limit);

    let mut slots: Vec<Option<ChunkHit>> = candidates.drain(..).map(Some).collect();
    let mut hits: Vec<Hit> = Vec::with_capacity(ranked.len());
    for (idx, score) in ranked {
        if let Some(c) = slots.get_mut(idx).and_then(Option::take) {
            hits.push(Hit { chunk: c, score });
        }
    }
    Ok(hits)
}

#[inline]
fn diversify_by_parent_in_place(
    ranked: &mut Vec<(usize, f32)>,
    candidates: &[ChunkHit],
    k: usize,
) {
    let mut parents_seen: HashMap<u32, u32> = HashMap::with_capacity(ranked.len());
    for entry in ranked.iter_mut() {
        let parent = candidates.get(entry.0).and_then(|c| c.parent_sym_id);
        if let Some(p) = parent {
            let n = parents_seen.entry(p).or_insert(0);
            entry.1 -= (*n as f32) * DIVERSITY_PENALTY;
            *n += 1;
        }
    }
    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(Ordering::Equal));
    ranked.truncate(k);
}

pub async fn search_exhaustive(
    query: &str,
    engine: &EmbedEngine,
    db: &zti_store::Db,
    pid: &[u8; 32],
    opts: &SearchOpts<'_>,
) -> Result<Vec<Hit>> {
    let projects = db.projects_table().await?;
    let _project = projects
        .get(pid)
        .await?
        .ok_or_else(|| anyhow!("project not indexed"))?;

    let query_emb = engine.embed_query_async(query).await?;
    let raw_k = opts.limit.saturating_mul(KNN_OVERFETCH_MULT);

    let candidates = db
        .chunks_table(engine.dim())
        .await?
        .knn_exhaustive(&query_emb, raw_k, opts.languages, opts.path_glob)
        .await?;

    let mut hits: Vec<Hit> = Vec::with_capacity(candidates.len());
    for c in candidates {
        let score = c.score;
        hits.push(Hit { chunk: c, score });
    }
    Ok(hits)
}

async fn rebuild(
    chunks: &zti_store::chunks_table::ChunksTable,
    dim: usize,
    params: &SearchParams,
) -> Result<zti_ann::hnsw::HnswGraph> {
    let n = params.indexed_chunks as usize;
    let mut flat: Vec<f32> = Vec::with_capacity(n * dim);
    let mut chunk_ids: Vec<[u8; 16]> = Vec::with_capacity(n);

    chunks
        .iter_vectors(|id, v| {
            flat.extend_from_slice(v);
            chunk_ids.push(*id);
        })
        .await?;

    Ok(zti_ann::hnsw::HnswGraph::build(dim, &flat, chunk_ids, params))
}
