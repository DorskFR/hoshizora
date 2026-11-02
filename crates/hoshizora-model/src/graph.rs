//! The resolved, language-neutral code graph served by the API.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::facts::{DefaultKind, DomainValue, Language, Param, Span, Visibility};

/// Stable node id. Code nodes: `<kind>:<path>[#<local_name>]`, e.g.
/// `fn:crates/api/src/sessions.rs#SessionStore::create`, `file:webui/src/api.ts`,
/// `mod:crates/api`. Synthetic nodes: `route:POST /api/sessions`, `table:sessions`,
/// `host:api.github.com`, `topic:session.spawn`, `ext:sqlx`.
pub type NodeId = String;

pub const GRAPH_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Graph {
    pub meta: GraphMeta,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    #[serde(default)]
    pub states: States,
}

/// Declared value domains and the branch sites linked to them. Both lists are sorted
/// (domains by id, sites by function then line).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct States {
    pub domains: Vec<StateDomain>,
    pub sites: Vec<StateSite>,
}

/// Id: `domain:<path>#<name>`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StateDomain {
    pub id: String,
    pub name: String,
    pub file: String,
    pub language: Language,
    pub values: Vec<DomainValue>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StateSite {
    pub function: NodeId,
    pub line: u32,
    pub subject: String,
    pub domain: String,
    /// Values with a branch, as written (member name or literal).
    pub handled: Vec<String>,
    /// Domain member names no branch covers.
    pub missing: Vec<String>,
    pub default: DefaultKind,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct GraphMeta {
    pub version: u32,
    pub root: String,
    pub generated_at: String,
    pub languages: Vec<Language>,
    pub files: u32,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    /// A directory-level unit: crate, Python package, npm package, Go package.
    Package,
    File,
    Type,
    Function,
    /// An HTTP route served by the code.
    Route,
    /// A database table or collection.
    Table,
    /// An external network host or base URL.
    Host,
    /// A message topic, queue, channel or protocol message type.
    Topic,
    /// A third-party package the code depends on.
    Dependency,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub id: NodeId,
    pub kind: NodeKind,
    pub name: String,
    pub qualified_name: String,
    pub language: Option<Language>,
    pub file: Option<String>,
    pub span: Option<Span>,
    /// Containing node: package for files, file for top-level symbols, type for methods.
    pub parent: Option<NodeId>,
    pub function: Option<FunctionInfo>,
    pub effects: Effects,
    pub entrypoint: Option<EntrypointKind>,
    /// Not reachable from any entrypoint and never observed at runtime.
    pub dead: bool,
    pub lines: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FunctionInfo {
    pub signature: String,
    pub params: Vec<Param>,
    pub returns: Option<String>,
    pub doc: Option<String>,
    pub is_async: bool,
    pub is_method: bool,
    pub visibility: Visibility,
    pub attributes: Vec<String>,
    pub cyclomatic: u32,
    pub branches: u32,
    pub loops: u32,
    pub max_loop_depth: u32,
    pub recursive: bool,
    pub complexity: Complexity,
    pub fan_in: u32,
    pub fan_out: u32,
}

/// Heuristic asymptotic estimate. `basis` says why, so nobody mistakes it for a proof.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Complexity {
    /// `O(1)`, `O(n)`, `O(n^2)`, `O(n log n)`, `O(2^n)`, `unknown`.
    pub time: String,
    pub space: String,
    pub basis: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntrypointKind {
    Main,
    HttpHandler,
    UiHandler,
    Cli,
    Job,
    Test,
    /// Public API of a library package.
    Export,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectKind {
    /// Outgoing network request.
    Network,
    DbRead,
    DbWrite,
    FsRead,
    FsWrite,
    /// Starts an OS process.
    Process,
    /// Starts concurrent work: task/thread/goroutine spawn, parallel iterators, gather.
    Spawn,
    /// Joins concurrent work: join handles, `Promise.all`, `WaitGroup.Wait`.
    Join,
    /// Acquires a lock or semaphore.
    Lock,
    Publish,
    Subscribe,
    Sleep,
    /// Blocking call inside async code.
    Blocking,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Effects {
    /// Effects caused by calls written directly in this node.
    pub direct: Vec<Effect>,
    /// Every effect kind reachable through callees, including direct ones.
    pub transitive: Vec<EffectKind>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Effect {
    pub kind: EffectKind,
    /// Table, host, path, topic or command when known.
    pub target: Option<String>,
    /// Callee that triggered it, as written.
    pub via: String,
    pub line: u32,
    pub loop_depth: u32,
    pub awaited: bool,
    /// Effect pack rule id that matched, e.g. `rust.sqlx.query`.
    pub rule: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    Contains,
    Calls,
    Imports,
    /// Caller starts the callee concurrently.
    Spawns,
    Implements,
    /// Client call → route it hits.
    Requests,
    /// Route → handler function.
    Handles,
    Reads,
    Writes,
    Publishes,
    Subscribes,
    /// Code → third-party dependency it uses.
    Uses,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    /// Resolved through an import or a qualified path.
    Exact,
    /// Matched by name within scope; several targets may share the name.
    Name,
    /// Pattern or string match (URLs, SQL, topics).
    Heuristic,
    /// Seen in runtime traces.
    Observed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Edge {
    pub from: NodeId,
    pub to: NodeId,
    pub kind: EdgeKind,
    pub confidence: Confidence,
    pub line: Option<u32>,
    pub loop_depth: u32,
    pub conditional: bool,
    pub awaited: bool,
    pub attrs: BTreeMap<String, String>,
}

impl Edge {
    pub fn new(
        from: impl Into<NodeId>,
        to: impl Into<NodeId>,
        kind: EdgeKind,
        confidence: Confidence,
    ) -> Self {
        Self {
            from: from.into(),
            to: to.into(),
            kind,
            confidence,
            line: None,
            loop_depth: 0,
            conditional: false,
            awaited: false,
            attrs: BTreeMap::new(),
        }
    }
}
