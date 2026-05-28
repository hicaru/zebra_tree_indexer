mod atom;
mod merge;
mod positions;

pub struct ChunkConfig {
    pub chunk_size: usize,
    pub min_chunk_size: usize,
    pub chunk_overlap: usize,
}

pub struct SubChunk {
    pub byte_start: usize,
    pub byte_end: usize,
    pub start_line: u32,
    pub end_line: u32,
}

use crate::merge::chunk_text;

pub fn split_text(
    source: &str,
    config: &ChunkConfig,
    _lang: Option<tree_sitter::Language>,
) -> Vec<SubChunk> {
    let min_chunk = config.min_chunk_size;
    let overlap = config.chunk_overlap.min(min_chunk);
    chunk_text(source, config.chunk_size, overlap, min_chunk)
}
