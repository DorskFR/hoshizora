//! Per-node aggregates built incrementally from store updates.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use hdrhistogram::Histogram;
use hoshizora_model::{NodeId, NodeStats, RuntimeSpan};

use crate::store::{StatsUpdate, Timing};

const CPU_ATTRS: [&str; 3] = ["thread.cpu_nanos", "process.cpu.time", "cpu.nanos"];
const ALLOC_ATTRS: [&str; 2] = ["alloc.bytes", "memory.alloc.bytes"];

#[derive(Debug)]
pub struct NodeAgg {
    count: u64,
    errors: u64,
    total_nanos: u64,
    self_nanos: u64,
    wait_nanos: u64,
    max_nanos: u64,
    durations: Histogram<u64>,
    /// Span end timestamps inside the rate window, oldest first.
    recent_ends: VecDeque<u64>,
    last_seen_unix_nanos: u64,
    cpu_nanos: Option<u64>,
    alloc_bytes: Option<u64>,
}

impl Default for NodeAgg {
    fn default() -> Self {
        Self {
            count: 0,
            errors: 0,
            total_nanos: 0,
            self_nanos: 0,
            wait_nanos: 0,
            max_nanos: 0,
            durations: Histogram::new(3).expect("3 significant figures is valid"),
            recent_ends: VecDeque::new(),
            last_seen_unix_nanos: 0,
            cpu_nanos: None,
            alloc_bytes: None,
        }
    }
}

impl NodeAgg {
    fn record(&mut self, span: &RuntimeSpan, timing: Timing) {
        let d = span.duration_nanos();
        self.count += 1;
        self.errors += u64::from(span.error);
        self.total_nanos += d;
        self.self_nanos += timing.self_nanos;
        self.wait_nanos += timing.wait_nanos;
        self.max_nanos = self.max_nanos.max(d);
        if self.durations.record(d.max(1)).is_err() {
            self.durations.saturating_record(d.max(1));
        }
        self.recent_ends.push_back(span.end_unix_nanos);
        self.last_seen_unix_nanos = self.last_seen_unix_nanos.max(span.end_unix_nanos);
        if let Some(v) = first_u64(span, &CPU_ATTRS) {
            *self.cpu_nanos.get_or_insert(0) += v;
        }
        if let Some(v) = first_u64(span, &ALLOC_ATTRS) {
            *self.alloc_bytes.get_or_insert(0) += v;
        }
    }

    fn retime(&mut self, before: Timing, after: Timing) {
        self.self_nanos = (self.self_nanos + after.self_nanos).saturating_sub(before.self_nanos);
        self.wait_nanos = (self.wait_nanos + after.wait_nanos).saturating_sub(before.wait_nanos);
    }

    fn trim_window(&mut self, now_unix_nanos: u64, window_secs: u64) {
        let cutoff = now_unix_nanos.saturating_sub(window_secs * 1_000_000_000);
        while self.recent_ends.front().is_some_and(|&t| t < cutoff) {
            self.recent_ends.pop_front();
        }
    }

    fn snapshot(&self, node: &str, now_unix_nanos: u64, window_secs: u64) -> NodeStats {
        let cutoff = now_unix_nanos.saturating_sub(window_secs * 1_000_000_000);
        let in_window = self.recent_ends.iter().filter(|&&t| t >= cutoff).count();
        NodeStats {
            node: node.to_string(),
            count: self.count,
            errors: self.errors,
            total_nanos: self.total_nanos,
            self_nanos: self.self_nanos,
            wait_nanos: self.wait_nanos,
            p50_nanos: self.percentile(50.0),
            p95_nanos: self.percentile(95.0),
            p99_nanos: self.percentile(99.0),
            max_nanos: self.max_nanos,
            rate_per_sec: in_window as f64 / window_secs.max(1) as f64,
            last_seen_unix_nanos: self.last_seen_unix_nanos,
            cpu_nanos: self.cpu_nanos,
            alloc_bytes: self.alloc_bytes,
        }
    }

    fn percentile(&self, p: f64) -> u64 {
        if self.count == 0 {
            0
        } else {
            self.durations.value_at_percentile(p).min(self.max_nanos)
        }
    }
}

fn first_u64(span: &RuntimeSpan, keys: &[&str]) -> Option<u64> {
    keys.iter().find_map(|k| {
        let v = span.attrs.get(*k)?;
        v.parse::<u64>().ok().or_else(|| {
            v.parse::<f64>()
                .ok()
                .filter(|f| *f >= 0.0)
                .map(|f| f as u64)
        })
    })
}

#[derive(Debug, Default)]
pub struct StatsTable {
    nodes: BTreeMap<NodeId, NodeAgg>,
    changed: BTreeSet<NodeId>,
}

impl StatsTable {
    pub fn apply(&mut self, updates: &[StatsUpdate]) {
        for u in updates {
            match u {
                StatsUpdate::Span { span, timing } => {
                    let Some(node) = &span.node else { continue };
                    self.nodes
                        .entry(node.clone())
                        .or_default()
                        .record(span, *timing);
                    self.changed.insert(node.clone());
                }
                StatsUpdate::Retime {
                    node,
                    before,
                    after,
                } => {
                    self.nodes
                        .entry(node.clone())
                        .or_default()
                        .retime(*before, *after);
                    self.changed.insert(node.clone());
                }
            }
        }
    }

    pub fn get(&self, node: &str, now_unix_nanos: u64, window_secs: u64) -> Option<NodeStats> {
        self.nodes
            .get(node)
            .map(|a| a.snapshot(node, now_unix_nanos, window_secs))
    }

    pub fn all(&self, now_unix_nanos: u64, window_secs: u64) -> Vec<NodeStats> {
        self.nodes
            .iter()
            .map(|(id, a)| a.snapshot(id, now_unix_nanos, window_secs))
            .collect()
    }

    pub fn take_changed(&mut self, now_unix_nanos: u64, window_secs: u64) -> Vec<NodeStats> {
        let changed = std::mem::take(&mut self.changed);
        changed
            .iter()
            .filter_map(|id| {
                let agg = self.nodes.get_mut(id)?;
                agg.trim_window(now_unix_nanos, window_secs);
                Some(agg.snapshot(id, now_unix_nanos, window_secs))
            })
            .collect()
    }

    pub fn observed_nodes(&self) -> impl Iterator<Item = &NodeId> {
        self.nodes.keys()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::join::tests::span;
    use hoshizora_model::SpanKind;

    fn recorded(
        node: &str,
        start: u64,
        end: u64,
        error: bool,
        attrs: &[(&str, &str)],
    ) -> StatsUpdate {
        let mut s = span("x", SpanKind::Internal, attrs);
        s.start_unix_nanos = start;
        s.end_unix_nanos = end;
        s.error = error;
        s.node = Some(node.into());
        let timing = Timing {
            self_nanos: s.duration_nanos(),
            wait_nanos: 0,
        };
        StatsUpdate::Span { span: s, timing }
    }

    const SEC: u64 = 1_000_000_000;

    #[test]
    fn percentiles_rate_and_resource_attrs() {
        let mut table = StatsTable::default();
        let updates: Vec<StatsUpdate> = (1..=100u64)
            .map(|i| {
                recorded(
                    "fn:a",
                    i * SEC,
                    i * SEC + i * 1_000_000,
                    i % 10 == 0,
                    &[("thread.cpu_nanos", "10"), ("alloc.bytes", "2.0")],
                )
            })
            .collect();
        table.apply(&updates);
        let now = 100 * SEC + 1;
        let s = table.get("fn:a", now, 60).unwrap();
        assert_eq!(s.count, 100);
        assert_eq!(s.errors, 10);
        assert_eq!(s.max_nanos, 100_000_000);
        assert!(
            (49_000_000..=51_000_000).contains(&s.p50_nanos),
            "p50 {}",
            s.p50_nanos
        );
        assert!(
            (94_000_000..=96_000_000).contains(&s.p95_nanos),
            "p95 {}",
            s.p95_nanos
        );
        assert!(
            (98_000_000..=100_000_000).contains(&s.p99_nanos),
            "p99 {}",
            s.p99_nanos
        );
        assert_eq!(s.cpu_nanos, Some(1000));
        assert_eq!(s.alloc_bytes, Some(200));
        assert_eq!(s.last_seen_unix_nanos, 100 * SEC + 100_000_000);
        assert!(
            (s.rate_per_sec - 1.0).abs() < 0.02,
            "rate {}",
            s.rate_per_sec
        );
        assert!(
            table
                .get("fn:a", now + 1000 * SEC, 60)
                .unwrap()
                .rate_per_sec
                .abs()
                < f64::EPSILON
        );
        assert!(table.get("fn:missing", now, 60).is_none());
    }

    #[test]
    fn retime_adjusts_self_and_wait_and_marks_changed() {
        let mut table = StatsTable::default();
        table.apply(&[recorded("fn:a", 0, 100, false, &[])]);
        assert_eq!(table.take_changed(100, 60).len(), 1);
        assert!(table.take_changed(100, 60).is_empty());
        table.apply(&[StatsUpdate::Retime {
            node: "fn:a".into(),
            before: Timing {
                self_nanos: 100,
                wait_nanos: 0,
            },
            after: Timing {
                self_nanos: 40,
                wait_nanos: 30,
            },
        }]);
        let changed = table.take_changed(100, 60);
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0].self_nanos, 40);
        assert_eq!(changed[0].wait_nanos, 30);
        assert_eq!(changed[0].total_nanos, 100);
    }
}
