//! Graph access the server and CLI need, so handlers can be tested against a fake.

use hoshizora_core::Index;
use hoshizora_model::api::{Impact, SearchHit, Story, Subgraph};
use hoshizora_model::{Edge, EdgeKind, EffectKind, Graph, Insight, Node};
use hoshizora_runtime::NodeLookup;

pub trait GraphQueries: NodeLookup + 'static {
    fn from_graph(graph: Graph) -> Self
    where
        Self: Sized;
    fn graph(&self) -> &Graph;
    fn incoming(&self, id: &str) -> Vec<Edge>;
    fn outgoing(&self, id: &str) -> Vec<Edge>;
    fn flow(&self, from: &str, depth: u32, kinds: &[EdgeKind]) -> Option<Subgraph>;
    fn impact(&self, of: &str, depth: u32) -> Option<Impact>;
    fn story(&self, from: &str, depth: u32) -> Option<Story>;
    fn dead(&self) -> Vec<Node>;
    fn entrypoints(&self) -> Vec<Node>;
    fn with_effect(&self, kind: EffectKind, target: Option<&str>) -> Vec<Node>;
    fn search(&self, query: &str, limit: usize) -> Vec<SearchHit>;
    fn static_insights(&self) -> Vec<Insight>;
}

pub struct GraphIndex(pub Index);

impl GraphIndex {
    pub fn new(graph: Graph) -> Self {
        Self(Index::new(graph))
    }
}

impl NodeLookup for GraphIndex {
    fn node(&self, id: &str) -> Option<&Node> {
        self.0.node(id)
    }
    fn function_at(&self, file: &str, line: u32) -> Option<&Node> {
        self.0.function_at(file, line)
    }
    fn functions_named(&self, name: &str) -> Vec<&Node> {
        self.0.functions_named(name)
    }
    fn nodes(&self) -> &[Node] {
        &self.0.graph().nodes
    }
}

impl GraphQueries for GraphIndex {
    fn from_graph(graph: Graph) -> Self {
        Self::new(graph)
    }
    fn graph(&self) -> &Graph {
        self.0.graph()
    }
    fn incoming(&self, id: &str) -> Vec<Edge> {
        self.0.incoming(id).into_iter().cloned().collect()
    }
    fn outgoing(&self, id: &str) -> Vec<Edge> {
        self.0.outgoing(id).into_iter().cloned().collect()
    }
    fn flow(&self, from: &str, depth: u32, kinds: &[EdgeKind]) -> Option<Subgraph> {
        self.0.flow(from, depth, kinds)
    }
    fn impact(&self, of: &str, depth: u32) -> Option<Impact> {
        self.0.impact(of, depth)
    }
    fn story(&self, from: &str, depth: u32) -> Option<Story> {
        self.0.story(from, depth)
    }
    fn dead(&self) -> Vec<Node> {
        self.0.dead().into_iter().cloned().collect()
    }
    fn entrypoints(&self) -> Vec<Node> {
        self.0.entrypoints().into_iter().cloned().collect()
    }
    fn with_effect(&self, kind: EffectKind, target: Option<&str>) -> Vec<Node> {
        self.0
            .with_effect(kind, target)
            .into_iter()
            .cloned()
            .collect()
    }
    fn search(&self, query: &str, limit: usize) -> Vec<SearchHit> {
        self.0.search(query, limit)
    }
    fn static_insights(&self) -> Vec<Insight> {
        self.0.static_insights()
    }
}

/// Parses a comma-separated list of snake_case edge kinds (`calls,spawns`).
pub fn parse_edge_kinds(spec: &str) -> anyhow::Result<Vec<EdgeKind>> {
    spec.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| {
            serde_json::from_value(serde_json::Value::String(s.to_string()))
                .map_err(|_| anyhow::anyhow!("unknown edge kind: {s}"))
        })
        .collect()
}

pub fn parse_effect_kind(spec: &str) -> anyhow::Result<EffectKind> {
    serde_json::from_value(serde_json::Value::String(spec.trim().to_string()))
        .map_err(|_| anyhow::anyhow!("unknown effect kind: {spec}"))
}
