//! Insights that need runtime data. Static ones come from `Index::static_insights`.

use std::collections::{BTreeMap, HashMap, HashSet};

use hoshizora_model::{Insight, NodeId, NodeKind};

use crate::NodeLookup;
use crate::config::RuntimeConfig;
use crate::stats::StatsTable;
use crate::store::{TraceStore, is_wait, union_within};

pub fn runtime_insights(
    config: &RuntimeConfig,
    lookup: &dyn NodeLookup,
    store: &TraceStore,
    stats: &StatsTable,
    first_ingest_unix_nanos: Option<u64>,
    now_unix_nanos: u64,
) -> Vec<Insight> {
    let all = stats.all(now_unix_nanos, config.rate_window_secs);
    let mut out = Vec::new();

    for s in &all {
        if s.rate_per_sec >= config.hot_rate_per_sec && s.p95_nanos <= config.hot_p95_nanos {
            out.push(Insight::HotPath {
                node: s.node.clone(),
                rate_per_sec: s.rate_per_sec,
                p95_nanos: s.p95_nanos,
            });
        }
    }

    let self_p95 = self_p95_per_node(store);
    let mut sorted: Vec<u64> = self_p95.values().copied().collect();
    sorted.sort_unstable();
    let median = sorted
        .get(sorted.len().saturating_sub(1) / 2)
        .copied()
        .unwrap_or(0);
    for (node, p95) in &self_p95 {
        let Some(function) = lookup.node(node).and_then(|n| n.function.as_ref()) else {
            continue;
        };
        let relative = median > 0 && (*p95 as f64) >= config.inefficient_self_ratio * median as f64;
        let complex = function.cyclomatic >= config.inefficient_cyclomatic
            || function.complexity.time.contains('^')
            || function.max_loop_depth >= 2;
        if relative && complex {
            let mut reasons = vec![format!(
                "self p95 {:.1}x the median across nodes",
                *p95 as f64 / median as f64
            )];
            if function.cyclomatic >= config.inefficient_cyclomatic {
                reasons.push(format!("cyclomatic {}", function.cyclomatic));
            }
            if function.max_loop_depth >= 2 {
                reasons.push(format!("loop depth {}", function.max_loop_depth));
            }
            reasons.extend(function.complexity.basis.iter().cloned());
            out.push(Insight::Inefficient {
                node: node.clone(),
                self_p95_nanos: *p95,
                complexity: function.complexity.time.clone(),
                reasons,
            });
        }
    }

    let waits_on = wait_targets(store);
    for s in &all {
        if s.total_nanos == 0 {
            continue;
        }
        let ratio = s.wait_nanos as f64 / s.total_nanos as f64;
        if ratio > config.waiting_ratio {
            out.push(Insight::Waiting {
                node: s.node.clone(),
                wait_ratio: ratio,
                on: waits_on.get(&s.node).cloned().unwrap_or_default(),
            });
        }
    }

    out.extend(parallelisable(store));
    out.extend(io_in_loop(store, config.io_in_loop_calls));

    let old_enough = first_ingest_unix_nanos.is_some_and(|t| {
        now_unix_nanos.saturating_sub(t) >= config.never_observed_after_secs * 1_000_000_000
    });
    if old_enough {
        let observed: HashSet<&NodeId> = stats.observed_nodes().collect();
        for node in lookup.nodes() {
            if node.kind == NodeKind::Function && !node.dead && !observed.contains(&node.id) {
                out.push(Insight::NeverObserved {
                    node: node.id.clone(),
                });
            }
        }
    }
    out
}

fn self_p95_per_node(store: &TraceStore) -> BTreeMap<NodeId, u64> {
    let mut per_node: HashMap<&NodeId, Vec<u64>> = HashMap::new();
    for (span, timing) in store.spans_with_timing() {
        if let Some(node) = &span.node {
            per_node.entry(node).or_default().push(timing.self_nanos);
        }
    }
    per_node
        .into_iter()
        .map(|(node, mut v)| {
            v.sort_unstable();
            let idx = ((v.len() as f64 * 0.95).ceil() as usize).clamp(1, v.len()) - 1;
            (node.clone(), v[idx])
        })
        .collect()
}

fn wait_targets(store: &TraceStore) -> HashMap<NodeId, Vec<NodeId>> {
    let mut out: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
    for (parent, children) in store.parents_with_children() {
        let Some(pn) = &parent.node else { continue };
        let list = out.entry(pn.clone()).or_default();
        for c in children.iter().filter(|c| is_wait(c)) {
            if let Some(cn) = &c.node
                && !list.contains(cn)
            {
                list.push(cn.clone());
            }
        }
    }
    out.retain(|_, v| !v.is_empty());
    out
}

/// Under one parent span, ≥2 children (joined to distinct nodes) that never overlap in time.
/// Running them concurrently would save the sum of all but the longest.
fn parallelisable(store: &TraceStore) -> Vec<Insight> {
    let mut best: BTreeMap<NodeId, (Vec<NodeId>, u64)> = BTreeMap::new();
    for (parent, children) in store.parents_with_children() {
        let Some(pn) = &parent.node else { continue };
        let mut joined: Vec<_> = children.iter().filter(|c| c.node.is_some()).collect();
        if joined.len() < 2 {
            continue;
        }
        joined.sort_by_key(|c| c.start_unix_nanos);
        let overlaps = joined
            .windows(2)
            .any(|w| w[1].start_unix_nanos < w[0].end_unix_nanos);
        if overlaps {
            continue;
        }
        let mut nodes: Vec<NodeId> = joined.iter().filter_map(|c| c.node.clone()).collect();
        nodes.dedup();
        let distinct: HashSet<&NodeId> = nodes.iter().collect();
        if distinct.len() < 2 {
            continue;
        }
        let intervals: Vec<(u64, u64)> = joined
            .iter()
            .map(|c| (c.start_unix_nanos, c.end_unix_nanos))
            .collect();
        let total = union_within(&intervals, (parent.start_unix_nanos, parent.end_unix_nanos));
        let longest = joined.iter().map(|c| c.duration_nanos()).max().unwrap_or(0);
        let saved = total.saturating_sub(longest);
        if saved == 0 {
            continue;
        }
        let entry = best.entry(pn.clone()).or_insert((nodes.clone(), saved));
        if saved > entry.1 {
            *entry = (nodes, saved);
        }
    }
    best.into_iter()
        .map(|(node, (children, saved_nanos))| Insight::Parallelisable {
            node,
            children,
            saved_nanos,
        })
        .collect()
}

/// The same child node called more than `threshold` times under one parent span.
/// `observed_calls_per_parent` averages over every parent span that called it at all.
fn io_in_loop(store: &TraceStore, threshold: usize) -> Vec<Insight> {
    let mut per_pair: HashMap<(NodeId, NodeId), (u64, u64, usize)> = HashMap::new();
    for (parent, children) in store.parents_with_children() {
        let Some(pn) = &parent.node else { continue };
        let mut counts: HashMap<&NodeId, usize> = HashMap::new();
        for c in &children {
            if let Some(cn) = &c.node {
                *counts.entry(cn).or_default() += 1;
            }
        }
        for (cn, n) in counts {
            let e = per_pair.entry((pn.clone(), cn.clone())).or_default();
            e.0 += n as u64;
            e.1 += 1;
            e.2 = e.2.max(n);
        }
    }
    let mut out: Vec<Insight> = per_pair
        .into_iter()
        .filter_map(|((parent, child), (calls, parents, max_in_one))| {
            let per_parent = calls as f64 / parents as f64;
            (max_in_one > threshold).then_some(Insight::IoInLoop {
                node: parent,
                effect: child,
                line: 0,
                observed_calls_per_parent: Some(per_parent),
            })
        })
        .collect();
    out.sort_by(|a, b| format!("{a:?}").cmp(&format!("{b:?}")));
    out
}

#[cfg(test)]
mod tests {
    use hoshizora_model::{Complexity, FunctionInfo, SpanKind, Visibility};

    use super::*;
    use crate::join::tests::{FakeLookup, node, span};
    use crate::{Runtime, RuntimeConfig};

    fn s(
        trace: &str,
        id: &str,
        parent: Option<&str>,
        kind: SpanKind,
        start: u64,
        end: u64,
        attrs: &[(&str, &str)],
    ) -> hoshizora_model::RuntimeSpan {
        let mut sp = span(id, kind, attrs);
        sp.trace_id = trace.into();
        sp.span_id = id.into();
        sp.parent_span_id = parent.map(str::to_string);
        sp.start_unix_nanos = start;
        sp.end_unix_nanos = end;
        sp
    }

    fn lookup() -> FakeLookup {
        let mut heavy = node(
            "fn:a.rs#heavy",
            NodeKind::Function,
            Some("a.rs"),
            Some((1, 50)),
        );
        heavy.function = Some(FunctionInfo {
            signature: String::new(),
            params: vec![],
            returns: None,
            doc: None,
            is_async: false,
            is_method: false,
            visibility: Visibility::Public,
            attributes: vec![],
            cyclomatic: 15,
            branches: 0,
            loops: 2,
            max_loop_depth: 2,
            recursive: false,
            complexity: Complexity {
                time: "O(n^2)".into(),
                space: "O(1)".into(),
                basis: vec!["nested loops".into()],
            },
            fan_in: 0,
            fan_out: 0,
        });
        FakeLookup {
            nodes: vec![
                heavy,
                node(
                    "fn:a.rs#light",
                    NodeKind::Function,
                    Some("a.rs"),
                    Some((60, 70)),
                ),
                node(
                    "fn:a.rs#never",
                    NodeKind::Function,
                    Some("a.rs"),
                    Some((80, 90)),
                ),
                node("host:api", NodeKind::Host, None, None),
                node("table:t", NodeKind::Table, None, None),
            ],
        }
    }

    const MS: u64 = 1_000_000;

    #[test]
    fn parallelisable_waiting_and_io_in_loop() {
        let mut rt = Runtime::new(RuntimeConfig::default(), lookup());
        let spans = vec![
            s(
                "t1",
                "p",
                None,
                SpanKind::Server,
                0,
                100 * MS,
                &[("code.function.name", "light")],
            ),
            s(
                "t1",
                "c1",
                Some("p"),
                SpanKind::Client,
                0,
                40 * MS,
                &[("server.address", "api")],
            ),
            s(
                "t1",
                "c2",
                Some("p"),
                SpanKind::Client,
                40 * MS,
                80 * MS,
                &[("db.collection.name", "t")],
            ),
            s(
                "t2",
                "p",
                None,
                SpanKind::Server,
                0,
                100 * MS,
                &[("code.function.name", "light")],
            ),
            s(
                "t2",
                "c1",
                Some("p"),
                SpanKind::Client,
                0,
                40 * MS,
                &[("server.address", "api")],
            ),
            s(
                "t2",
                "c2",
                Some("p"),
                SpanKind::Client,
                10 * MS,
                80 * MS,
                &[("db.collection.name", "t")],
            ),
        ];
        let events = rt.ingest(spans, 1);
        assert_eq!(events.len(), 6);
        let insights = rt.insights(1);
        let par: Vec<_> = insights
            .iter()
            .filter(|i| matches!(i, Insight::Parallelisable { .. }))
            .collect();
        assert_eq!(
            par,
            vec![&Insight::Parallelisable {
                node: "fn:a.rs#light".into(),
                children: vec!["host:api".into(), "table:t".into()],
                saved_nanos: 40 * MS,
            }]
        );
        let waiting: Vec<_> = insights
            .iter()
            .filter(|i| matches!(i, Insight::Waiting { .. }))
            .collect();
        assert_eq!(waiting.len(), 1);
        match waiting[0] {
            Insight::Waiting {
                node,
                wait_ratio,
                on,
            } => {
                assert_eq!(node, "fn:a.rs#light");
                assert!((wait_ratio - 0.8).abs() < 1e-9, "{wait_ratio}");
                assert_eq!(on, &vec!["host:api".to_string(), "table:t".to_string()]);
            }
            _ => unreachable!(),
        }
        assert!(
            !insights
                .iter()
                .any(|i| matches!(i, Insight::NeverObserved { .. }))
        );

        let mut spans = vec![s(
            "t3",
            "p",
            None,
            SpanKind::Server,
            0,
            100 * MS,
            &[("code.function.name", "light")],
        )];
        for i in 0..5u64 {
            spans.push(s(
                "t3",
                &format!("q{i}"),
                Some("p"),
                SpanKind::Client,
                i * MS,
                i * MS + 1,
                &[("db.collection.name", "t")],
            ));
        }
        rt.ingest(spans, 1);
        let io: Vec<_> = rt
            .insights(1)
            .into_iter()
            .filter(|i| matches!(i, Insight::IoInLoop { .. }))
            .collect();
        assert_eq!(io.len(), 1);
        match &io[0] {
            Insight::IoInLoop {
                node,
                effect,
                observed_calls_per_parent,
                ..
            } => {
                assert_eq!(node, "fn:a.rs#light");
                assert_eq!(effect, "table:t");
                assert!((observed_calls_per_parent.unwrap() - 7.0 / 3.0).abs() < 1e-9);
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn hot_inefficient_and_never_observed() {
        let config = RuntimeConfig {
            hot_rate_per_sec: 1.0,
            ..RuntimeConfig::default()
        };
        let mut rt = Runtime::new(config, lookup());
        let sec = 1_000_000_000u64;
        let mut spans = Vec::new();
        for i in 0..120u64 {
            spans.push(s(
                &format!("l{i}"),
                "p",
                None,
                SpanKind::Internal,
                i * sec,
                i * sec + MS,
                &[("code.function.name", "light")],
            ));
        }
        for i in 0..10u64 {
            spans.push(s(
                &format!("h{i}"),
                "p",
                None,
                SpanKind::Internal,
                i * sec,
                i * sec + 50 * MS,
                &[("code.function.name", "heavy")],
            ));
        }
        rt.ingest(spans, 0);
        let now = 120 * sec;
        let insights = rt.insights(now);
        assert!(
            insights
                .iter()
                .any(|i| matches!(i, Insight::HotPath { node, .. } if node == "fn:a.rs#light"))
        );
        assert!(
            !insights
                .iter()
                .any(|i| matches!(i, Insight::HotPath { node, .. } if node == "fn:a.rs#heavy"))
        );
        let ineff: Vec<_> = insights
            .iter()
            .filter(|i| matches!(i, Insight::Inefficient { .. }))
            .collect();
        match ineff.as_slice() {
            [
                Insight::Inefficient {
                    node,
                    self_p95_nanos,
                    complexity,
                    reasons,
                },
            ] => {
                assert_eq!(node, "fn:a.rs#heavy");
                assert_eq!(*self_p95_nanos, 50 * MS);
                assert_eq!(complexity, "O(n^2)");
                assert!(reasons.iter().any(|r| r.contains("cyclomatic 15")));
            }
            other => panic!("{other:?}"),
        }
        let never: Vec<_> = insights
            .iter()
            .filter(|i| matches!(i, Insight::NeverObserved { .. }))
            .collect();
        assert_eq!(
            never,
            vec![&Insight::NeverObserved {
                node: "fn:a.rs#never".into()
            }]
        );
        assert!(matches!(
            insights.last(),
            Some(Insight::NeverObserved { .. })
        ));
        assert!(
            !rt.insights(30 * sec)
                .iter()
                .any(|i| matches!(i, Insight::NeverObserved { .. }))
        );
    }
}
