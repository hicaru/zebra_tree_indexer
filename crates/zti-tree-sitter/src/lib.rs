pub mod detect;
pub mod parse_args;
pub mod registry;

pub use detect::detect_from_path;
pub use parse_args::{parse_kinds, parse_language};
pub use registry::{Language, frontend_for};
