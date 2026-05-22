pub mod dsl;
pub mod tree;

pub const CHARS_PER_TOKEN: usize = 4;
pub const MANIFEST_CAP: usize = 2048;

pub use dsl::{
    AST_HEADER, DART_SEARCH_LEGEND, LEGEND_LINE, RUST_SEARCH_LEGEND, SOLIDITY_SEARCH_LEGEND,
    TS_SEARCH_LEGEND, build_children_by_parent, lang_search_legend, render_symbol_rich,
};
