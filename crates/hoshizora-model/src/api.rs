//! Response bodies of the HTTP API. See `docs/api.md`.

use serde::{Deserialize, Serialize};

use crate::graph::{Edge, EntrypointKind, Node, NodeId, StateColumn, StateDomain, StateSite};
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

/// `states` query: every domain (or the one named) with the sites branching on it, and the
/// table columns written with literals (filtered by `table.column`, table or linked domain).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct StatesReport {
    pub domains: Vec<DomainReport>,
    #[serde(default)]
    pub columns: Vec<StateColumn>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DomainReport {
    #[serde(flatten)]
    pub domain: StateDomain,
    pub sites: Vec<StateSite>,
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

/// A flow condensed for humans: the steps an entrypoint goes through, grouped by the process that
/// runs them. Pure helper functions are folded into the step that calls them.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Story {
    pub root: NodeId,
    /// Ordered left to right as the request travels: UI, services, then databases and externals.
    pub lanes: Vec<StoryLane>,
    /// Ordered by `seq`.
    pub steps: Vec<StoryStep>,
    pub links: Vec<StoryLink>,
    /// More steps existed than the cap allowed.
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoryLane {
    pub id: String,
    pub label: String,
    pub kind: LaneKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaneKind {
    Ui,
    Service,
    Worker,
    Database,
    Queue,
    External,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoryStep {
    pub node: NodeId,
    pub lane: String,
    /// Human label, e.g. `SpawnForm.submit`, `POST /sessions/spawn`, `sessions`.
    pub label: String,
    pub kind: crate::graph::NodeKind,
    /// Position in the sequence, starting at 0 for the root.
    pub seq: u32,
    /// Direct effects of this step and of the helpers folded into it.
    pub effects: Vec<crate::graph::Effect>,
    /// Helper functions folded into this step.
    pub folded: Vec<NodeId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoryLink {
    pub from: NodeId,
    pub to: NodeId,
    pub kind: crate::graph::EdgeKind,
    /// e.g. `POST /sessions/spawn`, `INSERT sessions`, `spawn`.
    pub label: Option<String>,
    pub seq: u32,
    /// Fire-and-forget or spawned rather than awaited.
    pub detached: bool,
    pub confidence: crate::graph::Confidence,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Project {
    /// URL-safe slug, e.g. `dorskfr-cctui`.
    pub id: String,
    pub name: String,
    pub source: ProjectSource,
    pub status: ProjectStatus,
    pub error: Option<String>,
    /// Commit the current graph was built from, when the source is a git checkout.
    pub commit: Option<String>,
    /// RFC 3339.
    pub indexed_at: Option<String>,
    pub nodes: usize,
    pub edges: usize,
    /// OTLP `service.name` values whose spans belong to this project.
    pub services: Vec<String>,
    /// Tag the clone follows instead of the declared ref, resolved from `seen_version`.
    #[serde(default)]
    pub tracked_ref: Option<String>,
    /// Ref the current graph was checked out at (`HEAD` of the remote when none is declared).
    #[serde(default)]
    pub indexed_ref: Option<String>,
    /// Last `service.version` carried by spans routed to this project.
    #[serde(default)]
    pub seen_version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProjectSource {
    Git { url: String, r#ref: Option<String> },
    Path { path: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectStatus {
    Cloning,
    Indexing,
    Ready,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct AddProject {
    pub url: String,
    pub r#ref: Option<String>,
    pub name: Option<String>,
    #[serde(default)]
    pub services: Vec<String>,
}

/// A clone deleted by `POST /api/projects/{project}/purge`; the project and its graph stay.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Purged {
    pub id: String,
    pub freed_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct PurgeReport {
    pub projects: Vec<Purged>,
    pub freed_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectStorage {
    pub id: String,
    /// `None` when the project has no clone (a path project, or a purged one).
    pub clone_bytes: Option<u64>,
    pub graph_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiskSpace {
    pub total_bytes: u64,
    pub free_bytes: u64,
}

/// Bytes the data dir holds per project, plus the filesystem it lives on.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Storage {
    pub projects: Vec<ProjectStorage>,
    pub clone_bytes: u64,
    pub graph_bytes: u64,
    pub disk: Option<DiskSpace>,
}
