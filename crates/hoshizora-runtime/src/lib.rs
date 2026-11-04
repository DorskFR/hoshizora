//! OTLP ingestion, span → node joining, per-node statistics and runtime insights.

pub mod config;
pub mod insights;
pub mod join;
pub mod otlp;
pub mod route;
pub mod stats;
pub mod store;

use std::sync::Arc;

use hoshizora_model::{Insight, LiveEvent, Node, NodeStats, RuntimeSpan};

pub use config::RuntimeConfig;
pub use join::JoinCache;
pub use stats::StatsSnapshot;
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
        let stats = stats::StatsTable::new(config.max_stats_nodes, config.rate_window_secs);
        Self {
            config,
            lookup,
            cache,
            store,
            stats,
            first_ingest_unix_nanos: None,
        }
    }

    pub fn config(&self) -> &RuntimeConfig {
        &self.config
    }

    pub fn lookup(&self) -> &L {
        &self.lookup
    }

    /// Replaces the graph (after a reindex): traces are dropped, aggregates survive for nodes
    /// the new graph still has.
    pub fn replace_lookup(&mut self, lookup: L) {
        self.cache = JoinCache::new(lookup.nodes());
        let kept = self.stats.to_snapshot(0);
        self.stats =
            stats::StatsTable::new(self.config.max_stats_nodes, self.config.rate_window_secs);
        self.stats.merge(&kept, |id| lookup.node(id).is_some());
        self.lookup = lookup;
        self.store = TraceStore::new(self.config.max_traces, self.config.max_spans);
        self.first_ingest_unix_nanos = None;
    }

    /// Aggregates in their on-disk form.
    pub fn export_stats(&self, now_unix_nanos: u64) -> StatsSnapshot {
        self.stats.to_snapshot(now_unix_nanos)
    }

    /// Adds persisted aggregates for nodes of this graph on top of what is already recorded.
    pub fn import_stats(&mut self, snapshot: &StatsSnapshot) -> usize {
        let before = self.stats.len();
        self.stats
            .merge(snapshot, |id| self.lookup.node(id).is_some());
        self.first_ingest_unix_nanos
            .get_or_insert(snapshot.saved_unix_nanos);
        self.stats.len() - before
    }

    /// Per observed node: histogram entries and rate-window entries.
    pub fn stats_footprints(&self) -> Vec<(String, (usize, usize))> {
        self.stats
            .footprints()
            .map(|(id, f)| (id.clone(), f))
            .collect()
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

    /// Whether `span`'s code location resolves to a function of this graph.
    pub fn locates(&self, span: &RuntimeSpan) -> bool {
        join::locates(span, &self.lookup, &self.cache)
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

#[cfg(test)]
mod tests {
    use super::*;
    use hoshizora_model::SpanKind;
    use join::tests::{graph, span};

    fn located(trace: &str, node: &str, start: u64, end: u64) -> RuntimeSpan {
        let mut s = span("op", SpanKind::Internal, &[]);
        s.trace_id = trace.into();
        s.span_id = format!("{trace}-{node}");
        s.start_unix_nanos = start;
        s.end_unix_nanos = end;
        s.node = Some(node.into());
        s
    }

    #[test]
    fn stats_survive_reindex_and_import_only_known_nodes() {
        let known = "fn:crates/worker/src/jobs.rs#create";
        let mut rt = Runtime::new(RuntimeConfig::default(), graph());
        rt.ingest(
            vec![located("t1", known, 0, 10), located("t2", "fn:gone", 0, 10)],
            10,
        );
        assert_eq!(rt.stats(10).len(), 2);
        assert_eq!(rt.traces(10).len(), 2);

        rt.replace_lookup(graph());
        let stats = rt.stats(10);
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].node, known);
        assert_eq!(stats[0].count, 1);
        assert!(rt.traces(10).is_empty());

        let snapshot = rt.export_stats(10);
        let mut fresh = Runtime::new(RuntimeConfig::default(), graph());
        fresh.ingest(vec![located("t3", known, 20, 30)], 30);
        assert_eq!(fresh.import_stats(&snapshot), 0);
        let merged = fresh.node_stats(known, 30).unwrap();
        assert_eq!(merged.count, 2);
        assert_eq!(merged.total_nanos, 20);
        assert_eq!(fresh.take_changed(30).len(), 1);
    }
}
