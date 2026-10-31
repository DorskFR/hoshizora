use std::path::Path;

use hoshizora_model::api::{Impact, SearchHit, Subgraph};
use hoshizora_model::{EdgeKind, Edge, EffectKind, FileFacts, Graph, Insight, Node};

/// Effect, route and client rules loaded from TOML packs.
#[derive(Debug, Clone, Default)]
pub struct Packs {}

impl Packs {
    /// Packs shipped in `packs/`, embedded at build time.
    pub fn builtin() -> Self {
        unimplemented!("lane B")
    }

    /// Adds every `*.toml` pack in `dir`, overriding builtin rules with the same id.
    pub fn load_dir(&mut self, dir: &Path) -> anyhow::Result<()> {
        let _ = dir;
        unimplemented!("lane B")
    }
}

pub fn build_graph(root: &Path, files: Vec<FileFacts>, packs: &Packs) -> Graph {
    let _ = (root, files, packs);
    unimplemented!("lane B")
}

pub fn save(graph: &Graph, path: &Path) -> anyhow::Result<()> {
    let _ = (graph, path);
    unimplemented!("lane B")
}

pub fn load(path: &Path) -> anyhow::Result<Graph> {
    let _ = path;
    unimplemented!("lane B")
}

/// Read-only query index over a graph.
pub struct Index {
    graph: Graph,
}

impl Index {
    pub fn new(graph: Graph) -> Self {
        Self { graph }
    }

    pub fn graph(&self) -> &Graph {
        &self.graph
    }

    pub fn node(&self, id: &str) -> Option<&Node> {
        let _ = id;
        unimplemented!("lane B")
    }

    pub fn incoming(&self, id: &str) -> Vec<&Edge> {
        let _ = id;
        unimplemented!("lane B")
    }

    pub fn outgoing(&self, id: &str) -> Vec<&Edge> {
        let _ = id;
        unimplemented!("lane B")
    }

    /// Innermost function node whose span contains `line` in `file`.
    pub fn function_at(&self, file: &str, line: u32) -> Option<&Node> {
        let _ = (file, line);
        unimplemented!("lane B")
    }

    /// Function nodes whose qualified or short name equals `name`.
    pub fn functions_named(&self, name: &str) -> Vec<&Node> {
        let _ = name;
        unimplemented!("lane B")
    }

    /// Everything `from` triggers, following `kinds` (all non-structural kinds when empty).
    pub fn flow(&self, from: &str, depth: u32, kinds: &[EdgeKind]) -> Option<Subgraph> {
        let _ = (from, depth, kinds);
        unimplemented!("lane B")
    }

    /// Everything that can reach `of`, and the entrypoints among them.
    pub fn impact(&self, of: &str, depth: u32) -> Option<Impact> {
        let _ = (of, depth);
        unimplemented!("lane B")
    }

    pub fn dead(&self) -> Vec<&Node> {
        unimplemented!("lane B")
    }

    pub fn entrypoints(&self) -> Vec<&Node> {
        unimplemented!("lane B")
    }

    pub fn with_effect(&self, kind: EffectKind, target: Option<&str>) -> Vec<&Node> {
        let _ = (kind, target);
        unimplemented!("lane B")
    }

    pub fn search(&self, query: &str, limit: usize) -> Vec<SearchHit> {
        let _ = (query, limit);
        unimplemented!("lane B")
    }

    /// Findings that need no runtime data: I/O in loops, blocking calls in async code,
    /// sequential independent awaits, high-complexity hot candidates.
    pub fn static_insights(&self) -> Vec<Insight> {
        unimplemented!("lane B")
    }
}
