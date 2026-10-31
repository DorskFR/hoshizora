//! OTLP ingestion, span → node joining, per-node statistics and runtime insights.

pub mod config;
pub mod insights;
pub mod join;
pub mod otlp;
pub mod stats;
pub mod store;

use std::sync::Arc;

use hoshizora_model::{Insight, LiveEvent, Node, NodeStats, RuntimeSpan};

pub use config::RuntimeConfig;
pub use join::JoinCache;
pub use store::{Trace, TraceStore};

/// Graph access the runtime join needs. Implemented for `hoshizora_core::Index` by the binary.
pub trait NodeLookup: Send + Sync {
    fn node(&self, id: &str) -> Option<&Node>;
    /// Innermost function whose span contains `line` in the graph-relative `file`.
    fn function_at(&self, file: &str, line: u32) -> Option<&Node>;
    /// Functions whose qualified or short name equals `name`.
    fn functions_named(&self, name: &str) -> Vec<&Node>;
    fn nodes(&self) -> &[Node];
}

impl<T: NodeLookup + ?Sized> NodeLookup for Arc<T> {
    fn node(&self, id: &str) -> Option<&Node> {
        (**self).node(id)
    }
    fn function_at(&self, file: &str, line: u32) -> Option<&Node> {
        (**self).function_at(file, line)
    }
    fn functions_named(&self, name: &str) -> Vec<&Node> {
        (**self).functions_named(name)
    }
    fn nodes(&self) -> &[Node] {
        (**self).nodes()
    }
}

/// Everything runtime: joins incoming spans, stores recent traces, keeps per-node stats.
pub struct Runtime<L: NodeLookup> {
    config: RuntimeConfig,
    lookup: L,
    cache: JoinCache,
    store: TraceStore,
    stats: stats::StatsTable,
    first_ingest_unix_nanos: Option<u64>,
}

impl<L: NodeLookup> Runtime<L> {
    pub fn new(config: RuntimeConfig, lookup: L) -> Self {
        let cache = JoinCache::new(lookup.nodes());
        let store = TraceStore::new(config.max_traces, config.max_spans);
        Self {
            config,
            lookup,
            cache,
            store,
            stats: stats::StatsTable::default(),
            first_ingest_unix_nanos: None,
        }
    }

    pub fn config(&self) -> &RuntimeConfig {
        &self.config
    }

    pub fn lookup(&self) -> &L {
        &self.lookup
    }

    /// Replaces the graph (after a reindex) and clears everything derived from the old one.
    pub fn replace_lookup(&mut self, lookup: L) {
        self.cache = JoinCache::new(lookup.nodes());
        self.lookup = lookup;
        self.store = TraceStore::new(self.config.max_traces, self.config.max_spans);
        self.stats = stats::StatsTable::default();
        self.first_ingest_unix_nanos = None;
    }

    /// Joins and records `spans`; returns them joined, in input order, as live events.
    pub fn ingest(&mut self, spans: Vec<RuntimeSpan>, now_unix_nanos: u64) -> Vec<LiveEvent> {
        self.first_ingest_unix_nanos.get_or_insert(now_unix_nanos);
        let mut events = Vec::with_capacity(spans.len());
        for mut span in spans {
            join::join(&mut span, &self.lookup, &self.cache);
            let updates = self.store.insert(span.clone());
            self.stats.apply(&updates);
            events.push(LiveEvent::Span { span });
        }
        events
    }

    pub fn stats(&self, now_unix_nanos: u64) -> Vec<NodeStats> {
        self.stats.all(now_unix_nanos, self.config.rate_window_secs)
    }

    pub fn node_stats(&self, node: &str, now_unix_nanos: u64) -> Option<NodeStats> {
        self.stats
            .get(node, now_unix_nanos, self.config.rate_window_secs)
    }

    /// Stats for nodes touched since the previous call.
    pub fn take_changed(&mut self, now_unix_nanos: u64) -> Vec<NodeStats> {
        self.stats
            .take_changed(now_unix_nanos, self.config.rate_window_secs)
    }

    pub fn traces(&self, limit: usize) -> Vec<Trace> {
        self.store.recent(limit)
    }

    pub fn trace(&self, trace_id: &str) -> Option<Trace> {
        self.store.get(trace_id)
    }

    /// Runtime findings merged with `static_insights` (from `Index::static_insights`).
    pub fn insights(&self, now_unix_nanos: u64, static_insights: &[Insight]) -> Vec<Insight> {
        insights::runtime_insights(
            &self.config,
            &self.lookup,
            &self.store,
            &self.stats,
            static_insights,
            self.first_ingest_unix_nanos,
            now_unix_nanos,
        )
    }
}

pub fn now_unix_nanos() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}
