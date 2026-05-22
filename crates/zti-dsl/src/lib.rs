pub mod batch;
pub mod chunking;
pub mod index;
pub mod model;
pub mod render;

pub use batch::resolve_symbol_bodies;
pub use chunking::{Chunk, DslChunker};
pub use index::{
    SourceFile, build_index, build_index_from_sources, files_by_language, glob_match_files,
};
pub use model::{FileEntry, ProjectIndex};
pub use render::{
    DART_SEARCH_LEGEND, LEGEND_LINE, RUST_SEARCH_LEGEND, SOLIDITY_SEARCH_LEGEND, TS_SEARCH_LEGEND,
    lang_search_legend,
};
pub use render::tree::AsciiTreeRenderer;
pub use zti_tree_sitter::{Language, detect_from_path};
pub use zti_ts_core::types::{Edge, EdgeKind, Kind, Symbol, Target};
