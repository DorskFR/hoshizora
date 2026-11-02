//! Graph building, effect packs, linkers and queries. Input is `FileFacts` from the adapters,
//! output is the language-neutral `Graph` served by the API.

mod build;
pub mod coverage;
pub mod diff;
mod index;
pub mod packs;
pub mod pattern;
pub mod sql;
mod store;
mod story;
pub mod url;

use std::path::Path;

use hoshizora_model::{FileFacts, Graph};

pub const INDEXER_VERSION: &str = env!("CARGO_PKG_VERSION");

pub use build::analysis::{ALL_EFFECTS, FLOW_KINDS};
pub use diff::{DiffReport, diff};
pub use index::{DEFAULT_FLOW_KINDS, FLOW_NODE_LIMIT, Index};
pub use packs::Packs;
pub use store::save_with;

pub fn build_graph(root: &Path, files: Vec<FileFacts>, packs: &Packs) -> Graph {
    build::build_graph(root, files, packs)
}

pub fn save(graph: &Graph, path: &Path) -> anyhow::Result<()> {
    store::save(graph, path)
}

pub fn load(path: &Path) -> anyhow::Result<Graph> {
    store::load(path)
}
