//! Runtime observations, normalised from OTLP and joined onto graph nodes.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::facts::DefaultKind;
use crate::graph::{Confidence, NodeId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpanKind {
    Internal,
    Server,
    Client,
    Producer,
    Consumer,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuntimeSpan {
    pub trace_id: String,
    pub span_id: String,
    pub parent_span_id: Option<String>,
    pub service: String,
    pub name: String,
    pub kind: SpanKind,
    pub start_unix_nanos: u64,
    pub end_unix_nanos: u64,
    pub error: bool,
    /// Attributes flattened to strings.
    pub attrs: BTreeMap<String, String>,
    /// Graph node this span was joined to, if any.
    pub node: Option<NodeId>,
    pub join: Option<JoinMethod>,
}

impl RuntimeSpan {
    pub fn duration_nanos(&self) -> u64 {
        self.end_unix_nanos.saturating_sub(self.start_unix_nanos)
    }
}

/// How a span was matched to a node, strongest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JoinMethod {
    /// `code.file.path` + `code.line.number` (or the legacy `code.filepath`/`code.lineno`).
    CodeLocation,
    /// `code.function.name` / `code.namespace` matched a qualified name.
    CodeFunction,
    /// `http.route` + method matched a route node.
    HttpRoute,
    /// `db.collection.name` or tables parsed from `db.query.text`.
    DbTable,
    /// `messaging.destination.name` matched a topic node.
    Messaging,
    /// `server.address` / `url.full` host matched a host node.
    Host,
    /// Span name equals a function's qualified or short name.
    SpanName,
}

impl JoinMethod {
    pub fn confidence(self) -> Confidence {
        match self {
            Self::CodeLocation | Self::CodeFunction | Self::HttpRoute => Confidence::Observed,
            _ => Confidence::Heuristic,
        }
    }
}

/// Aggregated runtime figures for one node over the retention window.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct NodeStats {
    pub node: NodeId,
    pub count: u64,
    pub errors: u64,
    pub total_nanos: u64,
    /// Total minus time covered by child spans.
    pub self_nanos: u64,
    /// Time spent in child client/db/messaging spans (waiting on something else).
    pub wait_nanos: u64,
    pub p50_nanos: u64,
    pub p95_nanos: u64,
    pub p99_nanos: u64,
    pub max_nanos: u64,
    pub rate_per_sec: f64,
    pub last_seen_unix_nanos: u64,
    /// `process.cpu.time`-style or `thread.cpu_nanos` attribute totals, when reported.
    pub cpu_nanos: Option<u64>,
    /// Allocated bytes reported as a span attribute (`alloc.bytes`), when reported.
    pub alloc_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Insight {
    /// High self time per call on a node with high static complexity. `self_p95_nanos` is
    /// `None` until runtime spans cover the node.
    Inefficient {
        node: NodeId,
        #[serde(default)]
        self_p95_nanos: Option<u64>,
        complexity: String,
        reasons: Vec<String>,
    },
    /// Cheap per call but called very often: a hot path that scales with architecture or resources.
    HotPath {
        node: NodeId,
        rate_per_sec: f64,
        p95_nanos: u64,
    },
    /// Most of the duration is spent waiting on child I/O spans.
    Waiting {
        node: NodeId,
        wait_ratio: f64,
        on: Vec<NodeId>,
    },
    /// Sibling child spans ran one after another with no overlap; they may run concurrently.
    Parallelisable {
        node: NodeId,
        children: Vec<NodeId>,
        saved_nanos: u64,
    },
    /// An I/O effect inside a loop (N+1 queries, request per item).
    IoInLoop {
        node: NodeId,
        effect: String,
        line: u32,
        observed_calls_per_parent: Option<f64>,
    },
    /// Statically reachable code never observed during the window.
    NeverObserved { node: NodeId },
    /// A branch on a declared domain misses some of its values and a catch-all swallows them.
    UnhandledValues {
        node: NodeId,
        line: u32,
        domain: String,
        missing: Vec<String>,
        default: DefaultKind,
    },
    /// A table read, then written or deleted from later in the same function with the task
    /// yielding in between and no transaction or lock taken first: the rows may have changed
    /// under the write.
    ReadThenWrite {
        node: NodeId,
        table: String,
        read_line: u32,
        write_line: u32,
        /// `writes` or `deletes`.
        write: String,
    },
}

/// One event on the live stream.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum LiveEvent {
    Span { span: RuntimeSpan },
    Stats { stats: Vec<NodeStats> },
}
