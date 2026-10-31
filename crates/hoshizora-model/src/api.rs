//! Response bodies of the HTTP API. See `docs/api.md`.

use serde::{Deserialize, Serialize};

use crate::graph::{Edge, EntrypointKind, Node, NodeId};
use crate::runtime::{Insight, NodeStats};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeDetail {
    pub node: Node,
    /// Source lines of the node's span, when it has one.
    pub source: Option<Source>,
    pub incoming: Vec<Edge>,
    pub outgoing: Vec<Edge>,
    pub stats: Option<NodeStats>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Source {
    pub file: String,
    pub start_line: u32,
    pub end_line: u32,
    pub text: String,
}

/// A subgraph returned by reachability queries.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Subgraph {
    pub root: NodeId,
    pub nodes: Vec<NodeId>,
    pub edges: Vec<Edge>,
    /// Ordered steps for the 2D flow panel: depth-first from `root`, one entry per visit.
    pub steps: Vec<FlowStep>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlowStep {
    pub node: NodeId,
    pub depth: u32,
    /// Edge kind that led here from the previous level; `None` for the root.
    pub via: Option<crate::graph::EdgeKind>,
    /// Swimlane: language/service/package the node belongs to.
    pub lane: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Impact {
    pub target: NodeId,
    /// Every node that can reach `target`.
    pub affected: Vec<NodeId>,
    pub entrypoints: Vec<AffectedEntrypoint>,
    pub edges: Vec<Edge>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AffectedEntrypoint {
    pub node: NodeId,
    pub kind: EntrypointKind,
    /// Shortest call path from the entrypoint to the target.
    pub path: Vec<NodeId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Insights {
    pub insights: Vec<Insight>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchHit {
    pub node: NodeId,
    pub name: String,
    pub qualified_name: String,
    pub kind: crate::graph::NodeKind,
    pub score: f32,
}
