pub mod client;
pub mod engine;
pub mod models;
pub mod provider;

pub use engine::RemoteEmbedEngine;
pub use models::{RemoteModelInfo, list_models, list_openrouter_models};
pub use provider::RemoteProvider;
