//! Bounded in-memory ring of recent traces, tolerant of spans arriving in any order.

use std::collections::{HashMap, VecDeque};

use hoshizora_model::{NodeId, RuntimeSpan, SpanKind};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Trace {
    pub trace_id: String,
    /// Name of the root span (the span without a known parent that started first).
    pub root: Option<String>,
    pub root_node: Option<NodeId>,
    pub start_unix_nanos: u64,
    pub duration_nanos: u64,
    pub spans: Vec<RuntimeSpan>,
}

/// Per-span timing that depends on children: `(self_nanos, wait_nanos)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Timing {
    pub self_nanos: u64,
    pub wait_nanos: u64,
}

/// What a newly inserted span changes in the aggregated stats.
#[derive(Debug, Clone, PartialEq)]
pub enum StatsUpdate {
    /// A span joined to `node` was recorded.
    Span { span: RuntimeSpan, timing: Timing },
    /// Children arrived for an already recorded span: its timing moved from `before` to `after`.
    Retime {
        node: NodeId,
        before: Timing,
        after: Timing,
    },
}

#[derive(Debug, Default)]
struct TraceEntry {
    spans: Vec<RuntimeSpan>,
    by_id: HashMap<String, usize>,
    children: HashMap<String, Vec<usize>>,
    timing: Vec<Timing>,
}

impl TraceEntry {
    fn compute_timing(&self, idx: usize) -> Timing {
        let span = &self.spans[idx];
        let Some(kids) = self.children.get(&span.span_id) else {
            return Timing {
                self_nanos: span.duration_nanos(),
                wait_nanos: 0,
            };
        };
        let all: Vec<(u64, u64)> = kids
            .iter()
            .map(|&k| (self.spans[k].start_unix_nanos, self.spans[k].end_unix_nanos))
            .collect();
        let waits: Vec<(u64, u64)> = kids
            .iter()
            .map(|&k| &self.spans[k])
            .filter(|c| is_wait(c))
            .map(|c| (c.start_unix_nanos, c.end_unix_nanos))
            .collect();
        let window = (span.start_unix_nanos, span.end_unix_nanos);
        Timing {
            self_nanos: span
                .duration_nanos()
                .saturating_sub(union_within(&all, window)),
            wait_nanos: union_within(&waits, window),
        }
    }

    fn to_trace(&self, trace_id: &str) -> Trace {
        let root = self
            .spans
            .iter()
            .filter(|s| {
                s.parent_span_id
                    .as_ref()
                    .is_none_or(|p| !self.by_id.contains_key(p))
            })
            .min_by_key(|s| s.start_unix_nanos);
        let start = self
            .spans
            .iter()
            .map(|s| s.start_unix_nanos)
            .min()
            .unwrap_or(0);
        let end = self
            .spans
            .iter()
            .map(|s| s.end_unix_nanos)
            .max()
            .unwrap_or(0);
        Trace {
            trace_id: trace_id.to_string(),
            root: root.map(|s| s.name.clone()),
            root_node: root.and_then(|s| s.node.clone()),
            start_unix_nanos: start,
            duration_nanos: end.saturating_sub(start),
            spans: self.spans.clone(),
        }
    }
}

/// A child whose duration is time the parent spent waiting on something else.
pub fn is_wait(span: &RuntimeSpan) -> bool {
    matches!(span.kind, SpanKind::Client | SpanKind::Producer)
        || span
            .attrs
            .keys()
            .any(|k| k.starts_with("db.") || k.starts_with("messaging."))
        || span.node.as_deref().is_some_and(|n| {
            n.starts_with("table:") || n.starts_with("host:") || n.starts_with("topic:")
        })
}

/// Length of the union of `intervals`, clipped to `window`.
pub fn union_within(intervals: &[(u64, u64)], window: (u64, u64)) -> u64 {
    let mut clipped: Vec<(u64, u64)> = intervals
        .iter()
        .map(|&(s, e)| (s.max(window.0), e.min(window.1)))
        .filter(|(s, e)| e > s)
        .collect();
    clipped.sort_unstable();
    let mut total = 0;
    let mut cur: Option<(u64, u64)> = None;
    for (s, e) in clipped {
        match cur {
            Some((cs, ce)) if s <= ce => cur = Some((cs, ce.max(e))),
            Some((cs, ce)) => {
                total += ce - cs;
                cur = Some((s, e));
            }
            None => cur = Some((s, e)),
        }
    }
    if let Some((cs, ce)) = cur {
        total += ce - cs;
    }
    total
}

#[derive(Debug)]
pub struct TraceStore {
    max_traces: usize,
    max_spans: usize,
    traces: HashMap<String, TraceEntry>,
    order: VecDeque<String>,
    span_count: usize,
}

impl TraceStore {
    pub fn new(max_traces: usize, max_spans: usize) -> Self {
        Self {
            max_traces: max_traces.max(1),
            max_spans: max_spans.max(1),
            traces: HashMap::new(),
            order: VecDeque::new(),
            span_count: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.traces.len()
    }

    pub fn is_empty(&self) -> bool {
        self.traces.is_empty()
    }

    pub fn span_count(&self) -> usize {
        self.span_count
    }

    pub fn insert(&mut self, span: RuntimeSpan) -> Vec<StatsUpdate> {
        let trace_id = span.trace_id.clone();
        if !self.traces.contains_key(&trace_id) {
            self.traces.insert(trace_id.clone(), TraceEntry::default());
            self.order.push_back(trace_id.clone());
        }
        let entry = self.traces.get_mut(&trace_id).expect("inserted above");
        let mut updates = Vec::new();
        if entry.by_id.contains_key(&span.span_id) {
            return updates;
        }
        let idx = entry.spans.len();
        entry.by_id.insert(span.span_id.clone(), idx);
        if let Some(parent) = &span.parent_span_id {
            entry.children.entry(parent.clone()).or_default().push(idx);
        }
        entry.spans.push(span);
        entry.timing.push(Timing::default());
        self.span_count += 1;

        let timing = entry.compute_timing(idx);
        entry.timing[idx] = timing;
        updates.push(StatsUpdate::Span {
            span: entry.spans[idx].clone(),
            timing,
        });

        if let Some(parent_idx) = entry.spans[idx]
            .parent_span_id
            .as_ref()
            .and_then(|p| entry.by_id.get(p))
            .copied()
        {
            let before = entry.timing[parent_idx];
            let after = entry.compute_timing(parent_idx);
            if before != after {
                entry.timing[parent_idx] = after;
                if let Some(node) = &entry.spans[parent_idx].node {
                    updates.push(StatsUpdate::Retime {
                        node: node.clone(),
                        before,
                        after,
                    });
                }
            }
        }

        self.evict();
        updates
    }

    fn evict(&mut self) {
        while self.traces.len() > self.max_traces || self.span_count > self.max_spans {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if let Some(entry) = self.traces.remove(&oldest) {
                self.span_count -= entry.spans.len();
            }
            if self.traces.is_empty() {
                break;
            }
        }
    }

    pub fn get(&self, trace_id: &str) -> Option<Trace> {
        self.traces.get(trace_id).map(|e| e.to_trace(trace_id))
    }

    /// Most recently started traces first.
    pub fn recent(&self, limit: usize) -> Vec<Trace> {
        self.order
            .iter()
            .rev()
            .take(limit)
            .filter_map(|id| self.get(id))
            .collect()
    }

    /// Every stored span with its computed timing.
    pub fn spans_with_timing(&self) -> impl Iterator<Item = (&RuntimeSpan, Timing)> {
        self.traces
            .values()
            .flat_map(|e| e.spans.iter().zip(e.timing.iter().copied()))
    }

    /// For each stored span: the span and its direct children.
    pub fn parents_with_children(&self) -> impl Iterator<Item = (&RuntimeSpan, Vec<&RuntimeSpan>)> {
        self.traces.values().flat_map(|e| {
            e.children.iter().filter_map(move |(parent, kids)| {
                let p = &e.spans[*e.by_id.get(parent)?];
                Some((p, kids.iter().map(|&k| &e.spans[k]).collect()))
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::join::tests::span;

    fn s(
        trace: &str,
        id: &str,
        parent: Option<&str>,
        kind: SpanKind,
        start: u64,
        end: u64,
        node: Option<&str>,
    ) -> RuntimeSpan {
        let mut sp = span(id, kind, &[]);
        sp.trace_id = trace.into();
        sp.span_id = id.into();
        sp.parent_span_id = parent.map(str::to_string);
        sp.start_unix_nanos = start;
        sp.end_unix_nanos = end;
        sp.node = node.map(str::to_string);
        sp
    }

    #[test]
    fn union_merges_overlaps_and_clips() {
        assert_eq!(union_within(&[(0, 10), (5, 15), (20, 25)], (0, 100)), 20);
        assert_eq!(union_within(&[(0, 10), (5, 15)], (8, 12)), 4);
        assert_eq!(union_within(&[], (0, 10)), 0);
        assert_eq!(union_within(&[(50, 60)], (0, 10)), 0);
    }

    #[test]
    fn self_and_wait_with_overlapping_children_in_any_order() {
        let mut store = TraceStore::new(10, 100);
        let c1 = s(
            "t",
            "c1",
            Some("p"),
            SpanKind::Client,
            10,
            40,
            Some("host:x"),
        );
        let c2 = s(
            "t",
            "c2",
            Some("p"),
            SpanKind::Internal,
            30,
            60,
            Some("fn:b"),
        );
        let p = s("t", "p", None, SpanKind::Server, 0, 100, Some("fn:a"));

        let u = store.insert(c1);
        assert!(
            matches!(u.as_slice(), [StatsUpdate::Span { timing, .. }] if timing.self_nanos == 30)
        );
        let u = store.insert(p);
        assert!(
            matches!(u.as_slice(), [StatsUpdate::Span { timing, .. }] if *timing == Timing { self_nanos: 70, wait_nanos: 30 })
        );
        let u = store.insert(c2);
        assert_eq!(u.len(), 2);
        assert_eq!(
            u[1],
            StatsUpdate::Retime {
                node: "fn:a".into(),
                before: Timing {
                    self_nanos: 70,
                    wait_nanos: 30
                },
                after: Timing {
                    self_nanos: 50,
                    wait_nanos: 30
                },
            }
        );
        let trace = store.get("t").unwrap();
        assert_eq!(trace.root.as_deref(), Some("p"));
        assert_eq!(trace.root_node.as_deref(), Some("fn:a"));
        assert_eq!(trace.duration_nanos, 100);
        assert_eq!(trace.spans.len(), 3);
    }

    #[test]
    fn duplicate_span_ignored() {
        let mut store = TraceStore::new(10, 100);
        store.insert(s("t", "a", None, SpanKind::Internal, 0, 1, None));
        assert!(
            store
                .insert(s("t", "a", None, SpanKind::Internal, 0, 1, None))
                .is_empty()
        );
        assert_eq!(store.span_count(), 1);
    }

    #[test]
    fn evicts_oldest_traces_by_count_and_spans() {
        let mut store = TraceStore::new(2, 100);
        for i in 0..3 {
            store.insert(s(
                &format!("t{i}"),
                "a",
                None,
                SpanKind::Internal,
                i,
                i + 1,
                None,
            ));
        }
        assert_eq!(store.len(), 2);
        assert!(store.get("t0").is_none());
        assert_eq!(
            store
                .recent(5)
                .iter()
                .map(|t| t.trace_id.as_str())
                .collect::<Vec<_>>(),
            ["t2", "t1"]
        );

        let mut store = TraceStore::new(100, 3);
        for i in 0..4 {
            store.insert(s(
                "big",
                &format!("s{i}"),
                None,
                SpanKind::Internal,
                0,
                1,
                None,
            ));
            store.insert(s(
                &format!("t{i}"),
                "a",
                None,
                SpanKind::Internal,
                0,
                1,
                None,
            ));
        }
        assert!(store.span_count() <= 3);
        assert!(store.get("big").is_none());
    }
}
