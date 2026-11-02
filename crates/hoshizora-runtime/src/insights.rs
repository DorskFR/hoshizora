//! Runtime insights, merged with the static ones from `Index::static_insights`.
//!
//! Runtime data confirms and measures static candidates (Parallelisable, IoInLoop, Inefficient)
//! and adds what only traces can show (HotPath, Waiting, NeverObserved). Output order: entries
//! with runtime evidence, then static-only ones, then NeverObserved.

use std::collections::{BTreeMap, HashMap, HashSet};

use hoshizora_model::{EffectKind, Insight, NodeId, NodeKind, RuntimeSpan};

use crate::NodeLookup;
use crate::config::RuntimeConfig;
use crate::stats::StatsTable;
use crate::store::{TraceStore, is_wait, union_within};

pub fn runtime_insights(
    config: &RuntimeConfig,
    lookup: &dyn NodeLookup,
    store: &TraceStore,
    stats: &StatsTable,
    static_insights: &[Insight],
    first_ingest_unix_nanos: Option<u64>,
    now_unix_nanos: u64,
) -> Vec<Insight> {
    let all = stats.all(now_unix_nanos, config.rate_window_secs);
    let p50_by_node: HashMap<&NodeId, u64> = all.iter().map(|s| (&s.node, s.p50_nanos)).collect();
    let self_p95 = self_p95_per_node(store);
    let io_calls = io_calls_per_parent(store, lookup, config.io_in_loop_calls);

    let mut strong: Vec<Insight> = Vec::new();
    let mut weak: Vec<Insight> = Vec::new();
    let mut static_only: Vec<Insight> = Vec::new();
    let mut merged: HashSet<(&'static str, NodeId)> = HashSet::new();

    for insight in static_insights {
        match insight {
            Insight::Parallelisable { node, children, .. } => {
                match confirm_parallelisable(store, lookup, node, children) {
                    Some(saved) => {
                        let parent_p50 = p50_by_node.get(node).copied().unwrap_or(0);
                        let out = Insight::Parallelisable {
                            node: node.clone(),
                            children: children.clone(),
                            saved_nanos: saved,
                        };
                        merged.insert(("parallelisable", node.clone()));
                        if saved > 1_000_000 && saved as f64 > 0.05 * parent_p50 as f64 {
                            strong.push(out);
                        } else {
                            weak.push(out);
                        }
                    }
                    None => static_only.push(insight.clone()),
                }
            }
            Insight::IoInLoop {
                node, effect, line, ..
            } => {
                let observed = io_calls.get(node).and_then(|children| {
                    children
                        .iter()
                        .find(|(child, _)| {
                            lookup
                                .node(child)
                                .is_some_and(|n| effect.contains(n.name.as_str()))
                        })
                        .or_else(|| children.iter().max_by(|a, b| a.1.total_cmp(&b.1)))
                        .map(|(_, per_parent)| *per_parent)
                });
                let out = Insight::IoInLoop {
                    node: node.clone(),
                    effect: effect.clone(),
                    line: *line,
                    observed_calls_per_parent: observed,
                };
                if observed.is_some() {
                    merged.insert(("io_in_loop", node.clone()));
                    strong.push(out);
                } else {
                    static_only.push(out);
                }
            }
            Insight::Inefficient {
                node,
                complexity,
                reasons,
                ..
            } => {
                let out = Insight::Inefficient {
                    node: node.clone(),
                    self_p95_nanos: self_p95.get(node).copied().unwrap_or(0),
                    complexity: complexity.clone(),
                    reasons: reasons.clone(),
                };
                merged.insert(("inefficient", node.clone()));
                if self_p95.contains_key(node) {
                    strong.push(out);
                } else {
                    static_only.push(out);
                }
            }
            other => static_only.push(other.clone()),
        }
    }

    for s in &all {
        if s.rate_per_sec >= config.hot_rate_per_sec && s.p95_nanos <= config.hot_p95_nanos {
            strong.push(Insight::HotPath {
                node: s.node.clone(),
                rate_per_sec: s.rate_per_sec,
                p95_nanos: s.p95_nanos,
            });
        }
    }

    let mut sorted: Vec<u64> = self_p95.values().copied().collect();
    sorted.sort_unstable();
    let median = sorted
        .get(sorted.len().saturating_sub(1) / 2)
        .copied()
        .unwrap_or(0);
    for (node, p95) in &self_p95 {
        if merged.contains(&("inefficient", node.clone())) {
            continue;
        }
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
            strong.push(Insight::Inefficient {
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
            strong.push(Insight::Waiting {
                node: s.node.clone(),
                wait_ratio: ratio,
                on: waits_on.get(&s.node).cloned().unwrap_or_default(),
            });
        }
    }

    for (parent, children) in &io_calls {
        if merged.contains(&("io_in_loop", parent.clone())) {
            continue;
        }
        for (child, per_parent) in children {
            strong.push(Insight::IoInLoop {
                node: parent.clone(),
                effect: child.clone(),
                line: 0,
                observed_calls_per_parent: Some(*per_parent),
            });
        }
    }

    let mut out = strong;
    out.extend(weak);
    out.extend(static_only);

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

/// Static candidate children are `callee:line` labels; they match an observed child span when
/// the span's node is named like the callee.
fn child_matches_label(lookup: &dyn NodeLookup, child: &RuntimeSpan, label: &str) -> bool {
    let Some(node_id) = &child.node else {
        return false;
    };
    let callee = label.rsplit_once(':').map_or(label, |(c, line)| {
        if line.chars().all(|ch| ch.is_ascii_digit()) {
            c
        } else {
            label
        }
    });
    let short = callee.rsplit(['.', ':']).next().unwrap_or(callee);
    if node_id == label || node_id == callee {
        return true;
    }
    lookup
        .node(node_id)
        .is_some_and(|n| n.name == short || n.qualified_name == callee)
}

/// Median over observed parent spans of the time the static candidate's children would save
/// when run concurrently: their union minus the longest one.
fn confirm_parallelisable(
    store: &TraceStore,
    lookup: &dyn NodeLookup,
    parent_node: &str,
    labels: &[String],
) -> Option<u64> {
    let mut savings = Vec::new();
    for (parent, children) in store.parents_with_children() {
        if parent.node.as_deref() != Some(parent_node) {
            continue;
        }
        let mut matched: Vec<&RuntimeSpan> = Vec::new();
        let mut matched_labels: HashSet<&str> = HashSet::new();
        for child in children {
            if let Some(label) = labels
                .iter()
                .find(|l| child_matches_label(lookup, child, l))
            {
                matched.push(child);
                matched_labels.insert(label.as_str());
            }
        }
        if matched_labels.len() < 2 {
            continue;
        }
        let intervals: Vec<(u64, u64)> = matched
            .iter()
            .map(|c| (c.start_unix_nanos, c.end_unix_nanos))
            .collect();
        let union = union_within(&intervals, (parent.start_unix_nanos, parent.end_unix_nanos));
        let longest = matched
            .iter()
            .map(|c| c.duration_nanos())
            .max()
            .unwrap_or(0);
        savings.push(union.saturating_sub(longest));
    }
    if savings.is_empty() {
        return None;
    }
    savings.sort_unstable();
    Some(savings[savings.len() / 2])
}

fn is_io_child(lookup: &dyn NodeLookup, child: &RuntimeSpan) -> bool {
    if is_wait(child) {
        return true;
    }
    child.node.as_deref().is_some_and(|id| {
        lookup.node(id).is_some_and(|n| {
            let io = |k: &EffectKind| {
                matches!(
                    k,
                    EffectKind::Network
                        | EffectKind::DbRead
                        | EffectKind::DbWrite
                        | EffectKind::DbDelete
                        | EffectKind::Publish
                )
            };
            n.effects.direct.iter().any(|e| io(&e.kind)) || n.effects.transitive.iter().any(io)
        })
    })
}

/// Per parent node: I/O child nodes called more than `threshold` times under one parent span,
/// with the average calls per parent span that called them at all.
fn io_calls_per_parent(
    store: &TraceStore,
    lookup: &dyn NodeLookup,
    threshold: usize,
) -> BTreeMap<NodeId, Vec<(NodeId, f64)>> {
    let mut per_pair: HashMap<(NodeId, NodeId), (u64, u64, usize)> = HashMap::new();
    for (parent, children) in store.parents_with_children() {
        let Some(pn) = &parent.node else { continue };
        let mut counts: HashMap<&NodeId, usize> = HashMap::new();
        for c in children.iter().filter(|c| is_io_child(lookup, c)) {
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
    let mut out: BTreeMap<NodeId, Vec<(NodeId, f64)>> = BTreeMap::new();
    for ((parent, child), (calls, parents, max_in_one)) in per_pair {
        if max_in_one > threshold {
            out.entry(parent)
                .or_default()
                .push((child, calls as f64 / parents as f64));
        }
    }
    for v in out.values_mut() {
        v.sort_by(|a, b| a.0.cmp(&b.0));
    }
    out
}

#[cfg(test)]
mod tests {
    use hoshizora_model::{Complexity, Effect, FunctionInfo, SpanKind, Visibility};

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
    ) -> RuntimeSpan {
        let mut sp = span(id, kind, attrs);
        sp.trace_id = trace.into();
        sp.span_id = id.into();
        sp.parent_span_id = parent.map(str::to_string);
        sp.start_unix_nanos = start;
        sp.end_unix_nanos = end;
        sp
    }

    fn function_info(cyclomatic: u32, depth: u32, time: &str) -> FunctionInfo {
        FunctionInfo {
            signature: String::new(),
            params: vec![],
            returns: None,
            doc: None,
            is_async: false,
            is_method: false,
            visibility: Visibility::Public,
            attributes: vec![],
            cyclomatic,
            branches: 0,
            loops: depth,
            max_loop_depth: depth,
            recursive: false,
            complexity: Complexity {
                time: time.into(),
                space: "O(1)".into(),
                basis: vec!["nested loops".into()],
            },
            fan_in: 0,
            fan_out: 0,
        }
    }

    fn lookup() -> FakeLookup {
        let mut heavy = node(
            "fn:a.rs#heavy",
            NodeKind::Function,
            Some("a.rs"),
            Some((1, 50)),
        );
        heavy.function = Some(function_info(15, 2, "O(n^2)"));
        let mut db_call = node(
            "fn:db.rs#count_rows",
            NodeKind::Function,
            Some("db.rs"),
            Some((1, 5)),
        );
        db_call.effects.direct.push(Effect {
            kind: EffectKind::DbRead,
            target: Some("t".into()),
            via: "sqlx::query".into(),
            line: 2,
            loop_depth: 0,
            awaited: true,
            rule: "rust.sqlx.query".into(),
        });
        db_call.effects.transitive.push(EffectKind::DbRead);
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
                node(
                    "fn:a.rs#pure",
                    NodeKind::Function,
                    Some("a.rs"),
                    Some((91, 95)),
                ),
                db_call,
                node("host:api", NodeKind::Host, None, None),
                node("table:t", NodeKind::Table, None, None),
            ],
        }
    }

    const MS: u64 = 1_000_000;

    fn parent(trace: &str, start: u64, end: u64) -> RuntimeSpan {
        s(
            trace,
            "p",
            None,
            SpanKind::Server,
            start,
            end,
            &[("code.function.name", "light")],
        )
    }

    #[test]
    fn parallelisable_only_confirms_static_candidates() {
        let mut rt = Runtime::new(RuntimeConfig::default(), lookup());
        let mut spans = Vec::new();
        for (i, (c1, c2)) in [
            ((0, 40), (40, 80)),
            ((0, 40), (10, 80)),
            ((0, 30), (30, 70)),
        ]
        .into_iter()
        .enumerate()
        {
            let t = format!("t{i}");
            spans.push(parent(&t, 0, 100 * MS));
            spans.push(s(
                &t,
                "c1",
                Some("p"),
                SpanKind::Client,
                c1.0 * MS,
                c1.1 * MS,
                &[("server.address", "api")],
            ));
            spans.push(s(
                &t,
                "c2",
                Some("p"),
                SpanKind::Client,
                c2.0 * MS,
                c2.1 * MS,
                &[("db.collection.name", "t")],
            ));
        }
        rt.ingest(spans, 1);

        let none: Vec<_> = rt
            .insights(1, &[])
            .into_iter()
            .filter(|i| matches!(i, Insight::Parallelisable { .. }))
            .collect();
        assert!(none.is_empty(), "no runtime-only parallelisable: {none:?}");

        let static_candidate = Insight::Parallelisable {
            node: "fn:a.rs#light".into(),
            children: vec!["host:api".into(), "t:12".into()],
            saved_nanos: 0,
        };
        let insights = rt.insights(1, std::slice::from_ref(&static_candidate));
        assert_eq!(
            insights[0],
            Insight::Parallelisable {
                node: "fn:a.rs#light".into(),
                children: vec!["host:api".into(), "t:12".into()],
                saved_nanos: 30 * MS,
            },
            "{insights:?}"
        );

        let unmatched = Insight::Parallelisable {
            node: "fn:a.rs#light".into(),
            children: vec!["foo:1".into(), "bar:2".into()],
            saved_nanos: 0,
        };
        let insights = rt.insights(1, std::slice::from_ref(&unmatched));
        let pos = insights.iter().position(|i| i == &unmatched).unwrap();
        assert!(
            insights[..pos]
                .iter()
                .all(|i| matches!(i, Insight::Waiting { .. })),
            "static-only ranks after runtime evidence: {insights:?}"
        );
    }

    #[test]
    fn weak_savings_rank_after_other_runtime_evidence() {
        let mut rt = Runtime::new(RuntimeConfig::default(), lookup());
        let spans = vec![
            parent("t", 0, 100 * MS),
            s(
                "t",
                "c1",
                Some("p"),
                SpanKind::Client,
                0,
                100_000,
                &[("server.address", "api")],
            ),
            s(
                "t",
                "c2",
                Some("p"),
                SpanKind::Client,
                100_000,
                200_000,
                &[("db.collection.name", "t")],
            ),
            s(
                "u",
                "p",
                None,
                SpanKind::Server,
                0,
                100 * MS,
                &[("code.function.name", "heavy")],
            ),
            s(
                "u",
                "c",
                Some("p"),
                SpanKind::Client,
                0,
                90 * MS,
                &[("server.address", "api")],
            ),
        ];
        rt.ingest(spans, 1);
        let candidate = Insight::Parallelisable {
            node: "fn:a.rs#light".into(),
            children: vec!["api:1".into(), "t:2".into()],
            saved_nanos: 0,
        };
        let insights = rt.insights(1, std::slice::from_ref(&candidate));
        assert_eq!(insights.len(), 2, "{insights:?}");
        assert!(
            matches!(&insights[0], Insight::Waiting { node, .. } if node == "fn:a.rs#heavy"),
            "{insights:?}"
        );
        assert_eq!(
            insights[1],
            Insight::Parallelisable {
                node: "fn:a.rs#light".into(),
                children: vec!["api:1".into(), "t:2".into()],
                saved_nanos: 100_000
            }
        );
    }

    #[test]
    fn waiting_names_wait_children() {
        let mut rt = Runtime::new(RuntimeConfig::default(), lookup());
        let spans = vec![
            parent("t", 0, 100 * MS),
            s(
                "t",
                "c1",
                Some("p"),
                SpanKind::Client,
                0,
                40 * MS,
                &[("server.address", "api")],
            ),
            s(
                "t",
                "c2",
                Some("p"),
                SpanKind::Client,
                40 * MS,
                80 * MS,
                &[("db.collection.name", "t")],
            ),
        ];
        rt.ingest(spans, 1);
        let insights = rt.insights(1, &[]);
        match insights
            .iter()
            .find(|i| matches!(i, Insight::Waiting { .. }))
        {
            Some(Insight::Waiting {
                node,
                wait_ratio,
                on,
            }) => {
                assert_eq!(node, "fn:a.rs#light");
                assert!((wait_ratio - 0.8).abs() < 1e-9, "{wait_ratio}");
                assert_eq!(on, &vec!["host:api".to_string(), "table:t".to_string()]);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn io_in_loop_counts_only_io_children_and_merges_static() {
        let mut rt = Runtime::new(RuntimeConfig::default(), lookup());
        let mut spans = vec![parent("t", 0, 100 * MS)];
        for i in 0..5u64 {
            spans.push(s(
                "t",
                &format!("q{i}"),
                Some("p"),
                SpanKind::Internal,
                i * MS,
                i * MS + 1,
                &[("code.function.name", "count_rows")],
            ));
            spans.push(s(
                "t",
                &format!("c{i}"),
                Some("p"),
                SpanKind::Internal,
                i * MS,
                i * MS + 1,
                &[("code.function.name", "pure")],
            ));
        }
        rt.ingest(spans, 1);

        let io: Vec<_> = rt
            .insights(1, &[])
            .into_iter()
            .filter(|i| matches!(i, Insight::IoInLoop { .. }))
            .collect();
        assert_eq!(
            io,
            vec![Insight::IoInLoop {
                node: "fn:a.rs#light".into(),
                effect: "fn:db.rs#count_rows".into(),
                line: 0,
                observed_calls_per_parent: Some(5.0),
            }]
        );

        let static_io = Insight::IoInLoop {
            node: "fn:a.rs#light".into(),
            effect: "db_read via count_rows".into(),
            line: 39,
            observed_calls_per_parent: None,
        };
        let insights = rt.insights(1, std::slice::from_ref(&static_io));
        let io: Vec<_> = insights
            .iter()
            .filter(|i| matches!(i, Insight::IoInLoop { .. }))
            .collect();
        assert_eq!(
            io,
            vec![&Insight::IoInLoop {
                node: "fn:a.rs#light".into(),
                effect: "db_read via count_rows".into(),
                line: 39,
                observed_calls_per_parent: Some(5.0),
            }]
        );
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
        let insights = rt.insights(now, &[]);
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
        assert_eq!(never.len(), 3);
        assert!(matches!(
            insights.last(),
            Some(Insight::NeverObserved { .. })
        ));
        assert!(
            !rt.insights(30 * sec, &[])
                .iter()
                .any(|i| matches!(i, Insight::NeverObserved { .. }))
        );

        let static_ineff = Insight::Inefficient {
            node: "fn:a.rs#heavy".into(),
            self_p95_nanos: 0,
            complexity: "O(n^2)".into(),
            reasons: vec!["nested loops (depth 2)".into()],
        };
        let insights = rt.insights(now, std::slice::from_ref(&static_ineff));
        let ineff: Vec<_> = insights
            .iter()
            .filter(|i| matches!(i, Insight::Inefficient { .. }))
            .collect();
        assert_eq!(
            ineff,
            vec![&Insight::Inefficient {
                node: "fn:a.rs#heavy".into(),
                self_p95_nanos: 50 * MS,
                complexity: "O(n^2)".into(),
                reasons: vec!["nested loops (depth 2)".into()],
            }],
            "static entry enriched, not duplicated"
        );
    }
}
