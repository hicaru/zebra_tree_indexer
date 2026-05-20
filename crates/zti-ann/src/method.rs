use serde::{Deserialize, Serialize};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SearchMethod {
    Flat,
    Usearch,
    IvfHnswSq,
}

impl SearchMethod {
    #[inline]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Flat => "flat",
            Self::Usearch => "usearch",
            Self::IvfHnswSq => "ivf_hnsw_sq",
        }
    }

    #[inline]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "flat" => Some(Self::Flat),
            "usearch" => Some(Self::Usearch),
            "ivf_hnsw_sq" => Some(Self::IvfHnswSq),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SearchParams {
    pub method: SearchMethod,
    pub indexed_chunks: u64,
    pub m: u32,
    pub ef_construction: u32,
    pub ef_search: u32,
    pub num_partitions: u32,
    pub nprobes: u32,
    pub refine_factor: u32,
}
