use hnsw_rs::prelude::{DistCosine, Hnsw};

use crate::method::SearchParams;

pub type ChunkId = [u8; 16];

pub struct HnswGraph {
    inner: Hnsw<'static, f32, DistCosine>,
    chunk_ids: Vec<ChunkId>,
    dim: usize,
    ef_search: usize,
}

impl HnswGraph {
    pub fn build(dim: usize, flat: &[f32], chunk_ids: Vec<ChunkId>, p: &SearchParams) -> Self {
        let n = chunk_ids.len();
        let capacity_hint = n.max(1_024);
        let inner = Hnsw::<f32, DistCosine>::new(
            p.m as usize,
            capacity_hint,
            16,
            p.ef_construction as usize,
            DistCosine,
        );
        for i in 0..n {
            let v = &flat[i * dim..(i + 1) * dim];
            inner.insert((v, i));
        }
        Self {
            inner,
            chunk_ids,
            dim,
            ef_search: p.ef_search as usize,
        }
    }

    #[inline]
    pub fn search(
        &self,
        query: &[f32],
        k: usize,
        ef_floor: usize,
        out: &mut Vec<(ChunkId, f32)>,
    ) {
        out.clear();
        let n = self.chunk_ids.len();
        if n == 0 || k == 0 || query.len() != self.dim {
            return;
        }
        let k = k.min(n);
        let ef = self.ef_search.max(ef_floor);
        out.reserve(k);
        for nb in self.inner.search(query, k, ef) {
            if let Some(id) = self.chunk_ids.get(nb.d_id) {
                // Embeddings are L2-normalised upstream (zti-embed/normalize.rs),
                // so 1.0 - cos_distance = cosine similarity in [-1, 1].
                out.push((*id, 1.0 - nb.distance));
            }
        }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.chunk_ids.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.chunk_ids.is_empty()
    }
}
