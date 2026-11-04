//! Per-node aggregates built incrementally from store updates.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::Path;

use hdrhistogram::Histogram;
use hoshizora_model::{NodeId, NodeStats, RuntimeSpan};
use serde::{Deserialize, Serialize};

use crate::store::{StatsUpdate, Timing};

const CPU_ATTRS: [&str; 3] = ["thread.cpu_nanos", "process.cpu.time", "cpu.nanos"];
const ALLOC_ATTRS: [&str; 2] = ["alloc.bytes", "memory.alloc.bytes"];
const NANOS_PER_SEC: u64 = 1_000_000_000;
/// Durations above this are clamped so the histogram stays small (2 sig figs, ~18KB at most).
const MAX_DURATION_NANOS: u64 = 3_600 * NANOS_PER_SEC;
const HISTOGRAM_SIGFIG: u8 = 2;

pub const SNAPSHOT_VERSION: u32 = 1;

fn new_histogram() -> Histogram<u32> {
    let mut h = Histogram::new_with_bounds(1, 2, HISTOGRAM_SIGFIG).expect("valid bounds");
    h.auto(true);
    h
}

#[derive(Debug)]
pub struct NodeAgg {
    count: u64,
    errors: u64,
    total_nanos: u64,
    self_nanos: u64,
    wait_nanos: u64,
    max_nanos: u64,
    durations: Histogram<u32>,
    /// Span ends per second inside the rate window: `(unix_secs, count)`, oldest first.
    recent_ends: VecDeque<(u64, u32)>,
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
            durations: new_histogram(),
            recent_ends: VecDeque::new(),
            last_seen_unix_nanos: 0,
            cpu_nanos: None,
            alloc_bytes: None,
        }
    }
}

impl NodeAgg {
    fn record(&mut self, span: &RuntimeSpan, timing: Timing, window_secs: u64) {
        let d = span.duration_nanos();
        self.count += 1;
        self.errors += u64::from(span.error);
        self.total_nanos += d;
        self.self_nanos += timing.self_nanos;
        self.wait_nanos += timing.wait_nanos;
        self.max_nanos = self.max_nanos.max(d);
        let d_clamped = d.clamp(1, MAX_DURATION_NANOS);
        if self.durations.record(d_clamped).is_err() {
            self.durations.saturating_record(d_clamped);
        }
        self.count_end(span.end_unix_nanos / NANOS_PER_SEC);
        self.last_seen_unix_nanos = self.last_seen_unix_nanos.max(span.end_unix_nanos);
        self.trim_window(self.last_seen_unix_nanos, window_secs);
        if let Some(v) = first_u64(span, &CPU_ATTRS) {
            *self.cpu_nanos.get_or_insert(0) += v;
        }
        if let Some(v) = first_u64(span, &ALLOC_ATTRS) {
            *self.alloc_bytes.get_or_insert(0) += v;
        }
    }

    fn count_end(&mut self, sec: u64) {
        let pos = self.recent_ends.iter().rposition(|&(s, _)| s <= sec);
        match pos {
            Some(i) if self.recent_ends[i].0 == sec => {
                self.recent_ends[i].1 = self.recent_ends[i].1.saturating_add(1);
            }
            Some(i) => self.recent_ends.insert(i + 1, (sec, 1)),
            None => self.recent_ends.push_front((sec, 1)),
        }
    }

    fn retime(&mut self, before: Timing, after: Timing) {
        self.self_nanos = (self.self_nanos + after.self_nanos).saturating_sub(before.self_nanos);
        self.wait_nanos = (self.wait_nanos + after.wait_nanos).saturating_sub(before.wait_nanos);
    }

    fn trim_window(&mut self, now_unix_nanos: u64, window_secs: u64) {
        let cutoff = now_unix_nanos.saturating_sub(window_secs * NANOS_PER_SEC) / NANOS_PER_SEC;
        while self.recent_ends.front().is_some_and(|&(s, _)| s < cutoff) {
            self.recent_ends.pop_front();
        }
    }

    fn snapshot(&self, node: &str, now_unix_nanos: u64, window_secs: u64) -> NodeStats {
        let cutoff = now_unix_nanos.saturating_sub(window_secs * NANOS_PER_SEC) / NANOS_PER_SEC;
        let in_window: u64 = self
            .recent_ends
            .iter()
            .filter(|&&(s, _)| s >= cutoff)
            .map(|&(_, n)| u64::from(n))
            .sum();
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

    /// Entries held by the duration histogram and the rate window.
    pub fn footprint(&self) -> (usize, usize) {
        (self.durations.distinct_values(), self.recent_ends.len())
    }

    fn to_persisted(&self) -> PersistedNode {
        PersistedNode {
            count: self.count,
            errors: self.errors,
            total_nanos: self.total_nanos,
            self_nanos: self.self_nanos,
            wait_nanos: self.wait_nanos,
            max_nanos: self.max_nanos,
            last_seen_unix_nanos: self.last_seen_unix_nanos,
            cpu_nanos: self.cpu_nanos,
            alloc_bytes: self.alloc_bytes,
            durations: self
                .durations
                .iter_recorded()
                .map(|v| (v.value_iterated_to(), v.count_at_value()))
                .collect(),
        }
    }

    fn merge(&mut self, p: &PersistedNode) {
        self.count += p.count;
        self.errors += p.errors;
        self.total_nanos += p.total_nanos;
        self.self_nanos += p.self_nanos;
        self.wait_nanos += p.wait_nanos;
        self.max_nanos = self.max_nanos.max(p.max_nanos);
        self.last_seen_unix_nanos = self.last_seen_unix_nanos.max(p.last_seen_unix_nanos);
        for &(value, count) in &p.durations {
            let value = value.clamp(1, MAX_DURATION_NANOS);
            if self.durations.record_n(value, count).is_err() {
                self.durations.saturating_record_n(value, count);
            }
        }
        if let Some(v) = p.cpu_nanos {
            *self.cpu_nanos.get_or_insert(0) += v;
        }
        if let Some(v) = p.alloc_bytes {
            *self.alloc_bytes.get_or_insert(0) += v;
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PersistedNode {
    pub count: u64,
    pub errors: u64,
    pub total_nanos: u64,
    pub self_nanos: u64,
    pub wait_nanos: u64,
    pub max_nanos: u64,
    pub last_seen_unix_nanos: u64,
    pub cpu_nanos: Option<u64>,
    pub alloc_bytes: Option<u64>,
    /// `(duration_nanos, count)` pairs at histogram resolution.
    pub durations: Vec<(u64, u32)>,
}

/// On-disk form of a project's aggregates; the rate window is not kept.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatsSnapshot {
    pub version: u32,
    pub saved_unix_nanos: u64,
    pub nodes: BTreeMap<NodeId, PersistedNode>,
}

impl StatsSnapshot {
    /// Writes to `path` atomically (temp file + rename).
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(self)?)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// `None` when the file is missing, unreadable or of another version.
    pub fn load(path: &Path) -> Option<Self> {
        let bytes = std::fs::read(path).ok()?;
        let snapshot: Self = match serde_json::from_slice(&bytes) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "ignoring unreadable stats file");
                return None;
            }
        };
        if snapshot.version != SNAPSHOT_VERSION {
            tracing::warn!(
                path = %path.display(),
                version = snapshot.version,
                "ignoring stats file of another version"
            );
            return None;
        }
        Some(snapshot)
    }
}

#[derive(Debug)]
pub struct StatsTable {
    max_nodes: usize,
    window_secs: u64,
    nodes: BTreeMap<NodeId, NodeAgg>,
    changed: BTreeSet<NodeId>,
}

impl Default for StatsTable {
    fn default() -> Self {
        let c = crate::RuntimeConfig::default();
        Self::new(c.max_stats_nodes, c.rate_window_secs)
    }
}

impl StatsTable {
    pub fn new(max_nodes: usize, window_secs: u64) -> Self {
        Self {
            max_nodes: max_nodes.max(1),
            window_secs,
            nodes: BTreeMap::new(),
            changed: BTreeSet::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    fn agg(&mut self, node: &NodeId) -> &mut NodeAgg {
        if !self.nodes.contains_key(node) && self.nodes.len() >= self.max_nodes {
            let stalest = self
                .nodes
                .iter()
                .min_by_key(|(_, a)| a.last_seen_unix_nanos)
                .map(|(id, _)| id.clone());
            if let Some(id) = stalest {
                self.nodes.remove(&id);
                self.changed.remove(&id);
            }
        }
        self.nodes.entry(node.clone()).or_default()
    }

    pub fn apply(&mut self, updates: &[StatsUpdate]) {
        for u in updates {
            match u {
                StatsUpdate::Span { span, timing } => {
                    let Some(node) = &span.node else { continue };
                    let window = self.window_secs;
                    self.agg(node).record(span, *timing, window);
                    self.changed.insert(node.clone());
                }
                StatsUpdate::Retime {
                    node,
                    before,
                    after,
                } => {
                    self.agg(node).retime(*before, *after);
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

    /// Per node: histogram entries and rate-window entries.
    pub fn footprints(&self) -> impl Iterator<Item = (&NodeId, (usize, usize))> {
        self.nodes.iter().map(|(id, a)| (id, a.footprint()))
    }

    pub fn to_snapshot(&self, now_unix_nanos: u64) -> StatsSnapshot {
        StatsSnapshot {
            version: SNAPSHOT_VERSION,
            saved_unix_nanos: now_unix_nanos,
            nodes: self
                .nodes
                .iter()
                .map(|(id, a)| (id.clone(), a.to_persisted()))
                .collect(),
        }
    }

    /// Adds `snapshot` into the live aggregates for nodes accepted by `keep`.
    pub fn merge(&mut self, snapshot: &StatsSnapshot, keep: impl Fn(&NodeId) -> bool) {
        for (id, node) in &snapshot.nodes {
            if !keep(id) {
                continue;
            }
            self.agg(id).merge(node);
            self.changed.insert(id.clone());
        }
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

    #[test]
    fn structures_stay_bounded_under_sustained_ingest() {
        let mut table = StatsTable::new(10_000, 60);
        for i in 0..200_000u64 {
            let start = i * 10_000_000;
            let d = 1_000 * (1 + (i * 7919) % 10_000_000);
            table.apply(&[recorded("fn:hot", start, start + d, false, &[])]);
        }
        let (hist, window) = table.nodes["fn:hot"].footprint();
        assert!(hist <= 4_608, "histogram entries {hist}");
        assert!(window <= 61, "rate window entries {window}");
        let s = table.get("fn:hot", 200_000 * 10_000_000, 60).unwrap();
        assert_eq!(s.count, 200_000);
        assert!(
            (s.rate_per_sec - 100.0).abs() < 2.0,
            "rate {}",
            s.rate_per_sec
        );
    }

    #[test]
    fn duration_histogram_is_clamped_to_an_hour() {
        let mut table = StatsTable::default();
        table.apply(&[recorded("fn:a", 0, 10 * MAX_DURATION_NANOS, false, &[])]);
        let s = table.get("fn:a", 0, 60).unwrap();
        assert_eq!(s.max_nanos, 10 * MAX_DURATION_NANOS);
        assert!(s.p50_nanos <= MAX_DURATION_NANOS * 101 / 100);
        assert!(table.nodes["fn:a"].footprint().0 <= 4_608);
    }

    #[test]
    fn evicts_least_recently_seen_node_past_the_cap() {
        let mut table = StatsTable::new(3, 60);
        for (i, node) in ["fn:a", "fn:b", "fn:c", "fn:d"].iter().enumerate() {
            let t = (i as u64 + 1) * SEC;
            table.apply(&[recorded(node, t, t + 1, false, &[])]);
        }
        table.apply(&[recorded("fn:b", 10 * SEC, 10 * SEC + 1, false, &[])]);
        table.apply(&[recorded("fn:e", 11 * SEC, 11 * SEC + 1, false, &[])]);
        let kept: Vec<_> = table.observed_nodes().cloned().collect();
        assert_eq!(kept, ["fn:b", "fn:d", "fn:e"]);
        assert_eq!(table.len(), 3);
    }

    #[test]
    fn snapshot_roundtrip_through_disk() {
        let mut table = StatsTable::default();
        let updates: Vec<StatsUpdate> = (1..=100u64)
            .map(|i| {
                recorded(
                    "fn:a",
                    i * SEC,
                    i * SEC + i * 1_000_000,
                    i % 10 == 0,
                    &[("thread.cpu_nanos", "10")],
                )
            })
            .collect();
        table.apply(&updates);
        table.apply(&[recorded("fn:b", 0, 5, true, &[])]);
        let dir = std::env::temp_dir().join(format!(
            "hoshizora-stats-{}-{}",
            std::process::id(),
            crate::now_unix_nanos()
        ));
        let path = dir.join("stats").join("p.json");

        let snapshot = table.to_snapshot(123);
        snapshot.save(&path).unwrap();
        assert!(!path.with_extension("json.tmp").exists());
        let loaded = StatsSnapshot::load(&path).unwrap();
        assert_eq!(loaded, snapshot);

        let mut restored = StatsTable::default();
        restored.merge(&loaded, |_| true);
        let now = 100 * SEC + 1;
        let before = table.get("fn:a", now, 60).unwrap();
        let after = restored.get("fn:a", now, 60).unwrap();
        assert_eq!(after.count, before.count);
        assert_eq!(after.errors, before.errors);
        assert_eq!(after.total_nanos, before.total_nanos);
        assert_eq!(after.self_nanos, before.self_nanos);
        assert_eq!(after.max_nanos, before.max_nanos);
        assert_eq!(after.cpu_nanos, before.cpu_nanos);
        assert_eq!(after.last_seen_unix_nanos, before.last_seen_unix_nanos);
        assert_eq!(after.p50_nanos, before.p50_nanos);
        assert_eq!(after.p95_nanos, before.p95_nanos);
        assert_eq!(after.p99_nanos, before.p99_nanos);
        assert!(after.rate_per_sec.abs() < f64::EPSILON);
        assert_eq!(restored.get("fn:b", now, 60).unwrap().errors, 1);
        assert_eq!(restored.take_changed(now, 60).len(), 2);

        let mut filtered = StatsTable::default();
        filtered.merge(&loaded, |id| id == "fn:b");
        assert_eq!(filtered.len(), 1);

        let mut doubled = restored;
        doubled.merge(&loaded, |_| true);
        assert_eq!(doubled.get("fn:a", now, 60).unwrap().count, 200);

        std::fs::write(&path, b"{not json").unwrap();
        assert!(StatsSnapshot::load(&path).is_none());
        let other = StatsSnapshot {
            version: SNAPSHOT_VERSION + 1,
            ..snapshot
        };
        std::fs::write(&path, serde_json::to_vec(&other).unwrap()).unwrap();
        assert!(StatsSnapshot::load(&path).is_none());
        assert!(StatsSnapshot::load(&dir.join("missing.json")).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
