//! Read-only query index over a graph.

use std::collections::{HashMap, HashSet, VecDeque};

use crate::build::resolve::plausible_call;

use hoshizora_model::api::{
    AffectedEntrypoint, DomainReport, FlowStep, Impact, SearchHit, StatesReport, Subgraph,
};
use hoshizora_model::{
    DefaultKind, Edge, EdgeKind, EffectKind, EntrypointKind, Flow, Graph, Insight, Node, NodeKind,
};

/// Edge kinds followed by `flow` and `impact` when none are given.
pub const DEFAULT_FLOW_KINDS: &[EdgeKind] = &[
    EdgeKind::Calls,
    EdgeKind::Spawns,
    EdgeKind::Requests,
    EdgeKind::Handles,
    EdgeKind::Reads,
    EdgeKind::Writes,
    EdgeKind::Publishes,
    EdgeKind::Subscribes,
];

pub const FLOW_NODE_LIMIT: usize = 2000;

pub struct Index {
    pub(crate) graph: Graph,
    by_id: HashMap<String, usize>,
    pub(crate) outgoing: Vec<Vec<usize>>,
    pub(crate) incoming: Vec<Vec<usize>>,
    /// File → function node indices sorted by span start.
    by_file: HashMap<String, Vec<usize>>,
    by_name: HashMap<String, Vec<usize>>,
    pub(crate) lanes: Vec<String>,
}

impl Index {
    pub fn new(graph: Graph) -> Self {
        let by_id: HashMap<String, usize> = graph
            .nodes
            .iter()
            .enumerate()
            .map(|(i, n)| (n.id.clone(), i))
            .collect();
        let mut outgoing = vec![Vec::new(); graph.nodes.len()];
        let mut incoming = vec![Vec::new(); graph.nodes.len()];
        for (ei, e) in graph.edges.iter().enumerate() {
            if let Some(&f) = by_id.get(&e.from) {
                outgoing[f].push(ei);
            }
            if let Some(&t) = by_id.get(&e.to) {
                incoming[t].push(ei);
            }
        }
        let mut by_file: HashMap<String, Vec<usize>> = HashMap::new();
        let mut by_name: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, n) in graph.nodes.iter().enumerate() {
            if n.kind == NodeKind::Function {
                if let Some(file) = &n.file {
                    by_file.entry(file.clone()).or_default().push(i);
                }
                by_name.entry(n.name.clone()).or_default().push(i);
                if n.qualified_name != n.name {
                    by_name.entry(n.qualified_name.clone()).or_default().push(i);
                }
            }
        }
        for list in by_file.values_mut() {
            list.sort_by_key(|&i| graph.nodes[i].span.map(|s| s.start_line).unwrap_or(0));
        }
        let lanes = (0..graph.nodes.len())
            .map(|i| lane_of(&graph, &by_id, i))
            .collect();
        Self {
            graph,
            by_id,
            outgoing,
            incoming,
            by_file,
            by_name,
            lanes,
        }
    }

    pub fn graph(&self) -> &Graph {
        &self.graph
    }

    pub(crate) fn idx(&self, id: &str) -> Option<usize> {
        self.by_id.get(id).copied()
    }

    pub fn node(&self, id: &str) -> Option<&Node> {
        self.idx(id).map(|i| &self.graph.nodes[i])
    }

    pub fn incoming(&self, id: &str) -> Vec<&Edge> {
        self.idx(id)
            .map(|i| {
                self.incoming[i]
                    .iter()
                    .map(|&e| &self.graph.edges[e])
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn outgoing(&self, id: &str) -> Vec<&Edge> {
        self.idx(id)
            .map(|i| {
                self.outgoing[i]
                    .iter()
                    .map(|&e| &self.graph.edges[e])
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Innermost function node whose span contains `line` in `file`.
    pub fn function_at(&self, file: &str, line: u32) -> Option<&Node> {
        self.by_file
            .get(file)?
            .iter()
            .map(|&i| &self.graph.nodes[i])
            .filter(|n| {
                n.span
                    .is_some_and(|s| s.start_line <= line && line <= s.end_line)
            })
            .min_by_key(|n| {
                n.span
                    .map(|s| s.end_line - s.start_line)
                    .unwrap_or(u32::MAX)
            })
    }

    /// Function nodes whose qualified or short name equals `name`.
    pub fn functions_named(&self, name: &str) -> Vec<&Node> {
        let mut seen = HashSet::new();
        self.by_name
            .get(name)
            .map(|v| {
                v.iter()
                    .filter(|&&i| seen.insert(i))
                    .map(|&i| &self.graph.nodes[i])
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Everything `from` triggers, following `kinds` (all non-structural kinds when empty).
    pub fn flow(&self, from: &str, depth: u32, kinds: &[EdgeKind]) -> Option<Subgraph> {
        let root = self.idx(from)?;
        let kinds = if kinds.is_empty() {
            DEFAULT_FLOW_KINDS
        } else {
            kinds
        };
        let mut visited: HashSet<usize> = HashSet::from([root]);
        let mut nodes = vec![root];
        let mut edges: Vec<usize> = Vec::new();
        let mut steps = vec![FlowStep {
            node: self.graph.nodes[root].id.clone(),
            depth: 0,
            via: None,
            lane: self.lanes[root].clone(),
        }];
        let mut truncated = false;
        let mut stack: Vec<(usize, u32)> = vec![(root, 0)];
        while let Some((i, d)) = stack.pop() {
            if d >= depth {
                continue;
            }
            let mut children: Vec<(usize, usize)> = self.outgoing[i]
                .iter()
                .copied()
                .filter(|&e| kinds.contains(&self.graph.edges[e].kind))
                .filter_map(|e| self.idx(&self.graph.edges[e].to).map(|t| (e, t)))
                .collect();
            children.sort_by_key(|&(e, _)| (self.graph.edges[e].line.unwrap_or(0), e));
            for (e, t) in children.into_iter().rev() {
                if visited.contains(&t) {
                    if !edges.contains(&e) {
                        edges.push(e);
                    }
                    continue;
                }
                if nodes.len() >= FLOW_NODE_LIMIT {
                    truncated = true;
                    break;
                }
                visited.insert(t);
                nodes.push(t);
                edges.push(e);
                steps.push(FlowStep {
                    node: self.graph.nodes[t].id.clone(),
                    depth: d + 1,
                    via: Some(self.graph.edges[e].kind),
                    lane: self.lanes[t].clone(),
                });
                stack.push((t, d + 1));
            }
        }
        Some(Subgraph {
            root: self.graph.nodes[root].id.clone(),
            nodes: nodes
                .iter()
                .map(|&i| self.graph.nodes[i].id.clone())
                .collect(),
            edges: edges.iter().map(|&e| self.graph.edges[e].clone()).collect(),
            steps,
            truncated,
        })
    }

    /// Everything that can reach `of`, and the entrypoints among them.
    pub fn impact(&self, of: &str, depth: u32) -> Option<Impact> {
        let target = self.idx(of)?;
        let mut parent: HashMap<usize, (usize, usize)> = HashMap::new();
        let mut dist: HashMap<usize, u32> = HashMap::from([(target, 0)]);
        let mut queue = VecDeque::from([target]);
        let mut order = Vec::new();
        let mut edges = Vec::new();
        while let Some(i) = queue.pop_front() {
            let d = dist[&i];
            if d >= depth {
                continue;
            }
            for &e in &self.incoming[i] {
                let edge = &self.graph.edges[e];
                if !DEFAULT_FLOW_KINDS.contains(&edge.kind) {
                    continue;
                }
                let Some(c) = self.idx(&edge.from) else {
                    continue;
                };
                if c == target {
                    continue;
                }
                edges.push(e);
                if dist.contains_key(&c) {
                    continue;
                }
                dist.insert(c, d + 1);
                parent.insert(c, (i, e));
                order.push(c);
                queue.push_back(c);
            }
        }
        edges.sort_unstable();
        edges.dedup();
        let mut entrypoints = Vec::new();
        for &c in &order {
            let Some(kind) = self.graph.nodes[c].entrypoint else {
                continue;
            };
            let mut path = vec![self.graph.nodes[c].id.clone()];
            let mut cur = c;
            while let Some(&(next, _)) = parent.get(&cur) {
                path.push(self.graph.nodes[next].id.clone());
                cur = next;
            }
            entrypoints.push(AffectedEntrypoint {
                node: self.graph.nodes[c].id.clone(),
                kind,
                path,
            });
        }
        entrypoints.sort_by(|a, b| {
            a.path
                .len()
                .cmp(&b.path.len())
                .then_with(|| a.node.cmp(&b.node))
        });
        Some(Impact {
            target: self.graph.nodes[target].id.clone(),
            affected: order
                .iter()
                .map(|&i| self.graph.nodes[i].id.clone())
                .collect(),
            entrypoints,
            edges: edges.iter().map(|&e| self.graph.edges[e].clone()).collect(),
        })
    }

    pub fn dead(&self) -> Vec<&Node> {
        self.graph.nodes.iter().filter(|n| n.dead).collect()
    }

    pub fn entrypoints(&self) -> Vec<&Node> {
        self.graph
            .nodes
            .iter()
            .filter(|n| n.entrypoint.is_some())
            .collect()
    }

    pub fn with_effect(&self, kind: EffectKind, target: Option<&str>) -> Vec<&Node> {
        self.graph
            .nodes
            .iter()
            .filter(|n| {
                n.effects.direct.iter().any(|e| {
                    e.kind == kind
                        && target.is_none_or(|t| {
                            e.target.as_deref().is_some_and(|et| {
                                et.split(',').any(|part| part == t) || et.contains(t)
                            })
                        })
                })
            })
            .collect()
    }

    /// Names match ignoring case, `_`, `-` and spaces, so `SpawnSession`, `spawn session` and
    /// `spawn_session` all find `spawn_session`. Entrypoints and routes rank first, tests last.
    pub fn search(&self, query: &str, limit: usize) -> Vec<SearchHit> {
        let q = fold_name(query);
        if q.is_empty() {
            return Vec::new();
        }
        let tokens = name_tokens(query);
        let mut hits: Vec<SearchHit> = self
            .graph
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(i, n)| {
                let name = fold_name(&n.name);
                let qualified = fold_name(&n.qualified_name);
                let file = n
                    .file
                    .as_deref()
                    .and_then(|f| f.rsplit('/').next())
                    .map(fold_name)
                    .unwrap_or_default();
                let score = if name == q {
                    1.0
                } else if qualified == q {
                    0.95
                } else if name.starts_with(&q) {
                    0.8
                } else if qualified.ends_with(&q) {
                    0.7
                } else if name.contains(&q) {
                    0.6
                } else if qualified.contains(&q) {
                    0.5
                } else if n.kind == NodeKind::Function && file.contains(&q) {
                    0.4
                } else if tokens.len() > 1 && tokens.iter().all(|t| qualified.contains(t)) {
                    0.45
                } else if is_subsequence(&q, &name) {
                    0.3
                } else {
                    return None;
                };
                let score = score - (n.name.len() as f32 * 0.0001).min(0.05);
                let score = if n.name.eq_ignore_ascii_case(query.trim()) {
                    score + 0.2
                } else {
                    score
                };
                let score = if self.is_test(i) {
                    score * 0.2
                } else if n.kind == NodeKind::Route
                    || n.entrypoint.is_some_and(|k| {
                        matches!(
                            k,
                            EntrypointKind::UiHandler
                                | EntrypointKind::HttpHandler
                                | EntrypointKind::Main
                                | EntrypointKind::Cli
                        )
                    })
                {
                    score + 0.15
                } else {
                    score
                };
                Some(SearchHit {
                    node: n.id.clone(),
                    name: n.name.clone(),
                    qualified_name: n.qualified_name.clone(),
                    kind: n.kind,
                    score,
                })
            })
            .collect();
        hits.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.qualified_name.cmp(&b.qualified_name))
        });
        hits.truncate(limit);
        hits
    }

    /// Every domain (or the one whose name or id is `domain`) with the sites branching on it,
    /// and every column (or those of that table, that `table.column`, or linked to that domain).
    pub fn states(&self, domain: Option<&str>) -> StatesReport {
        let domains: Vec<DomainReport> = self
            .graph
            .states
            .domains
            .iter()
            .filter(|d| domain.is_none_or(|q| d.name == q || d.id == q))
            .map(|d| DomainReport {
                domain: d.clone(),
                sites: self
                    .graph
                    .states
                    .sites
                    .iter()
                    .filter(|s| s.domain == d.id)
                    .cloned()
                    .collect(),
            })
            .collect();
        let columns = self
            .graph
            .states
            .columns
            .iter()
            .filter(|c| {
                domain.is_none_or(|q| {
                    c.table == q
                        || format!("{}.{}", c.table, c.column) == q
                        || c.domains
                            .iter()
                            .any(|id| domains.iter().any(|d| d.domain.id == *id))
                })
            })
            .cloned()
            .collect();
        StatesReport { domains, columns }
    }

    /// Findings that need no runtime data: I/O in loops, blocking calls in async code,
    /// sequential independent awaits, high-complexity hot candidates, a table read then
    /// written across an await without a transaction, branches on a domain whose catch-all
    /// silently swallows values they do not handle.
    pub fn static_insights(&self) -> Vec<Insight> {
        let mut out = Vec::new();
        for (i, n) in self.graph.nodes.iter().enumerate() {
            let Some(info) = &n.function else { continue };
            self.io_in_loop(i, &mut out);
            self.parallelisable(i, &mut out);
            self.blocking(i, info, &mut out);
            self.complexity_candidate(n, info, &mut out);
            self.read_then_write(i, &mut out);
        }
        out.extend(self.unhandled_values());
        out
    }

    /// A site that handles fewer values than it misses is a projection (`_ => None` on one
    /// variant of twenty), not a mapping that forgot some; a non-empty default with exactly
    /// one value missing is that value's branch; tests are skipped.
    pub(crate) fn silent_site(&self, s: &hoshizora_model::StateSite) -> bool {
        let silent = match s.default {
            DefaultKind::ReturnsEmpty => true,
            DefaultKind::Other => s.missing.len() > 1,
            DefaultKind::None | DefaultKind::Throws => false,
        };
        silent
            && s.handled.len() >= s.missing.len()
            && !self.idx(&s.function).is_some_and(|i| self.is_test(i))
    }

    fn unhandled_values(&self) -> Vec<Insight> {
        let name_of: HashMap<&str, &str> = self
            .graph
            .states
            .domains
            .iter()
            .map(|d| (d.id.as_str(), d.name.as_str()))
            .collect();
        self.graph
            .states
            .sites
            .iter()
            .filter(|s| self.silent_site(s))
            .map(|s| Insight::UnhandledValues {
                node: s.function.clone(),
                line: s.line,
                domain: name_of
                    .get(s.domain.as_str())
                    .map_or_else(|| s.domain.clone(), |n| (*n).to_owned()),
                missing: s.missing.clone(),
                default: s.default,
            })
            .collect()
    }

    /// Items in a function body that perform I/O: direct effects and calls into functions
    /// whose transitive effects include network or database work.
    fn io_items(&self, i: usize) -> Vec<IoItem> {
        let n = &self.graph.nodes[i];
        let mut items: Vec<IoItem> = n
            .effects
            .direct
            .iter()
            .filter(|e| IO_KINDS.contains(&e.kind))
            .map(|e| IoItem {
                label: e.via.clone(),
                line: e.line,
                loop_depth: e.loop_depth,
                awaited: e.awaited,
                conditional: false,
                root: effect_root(&e.via),
                is_method: e.via.contains('.') && !e.via.contains("::"),
            })
            .collect();
        for &ei in &self.outgoing[i] {
            let e = &self.graph.edges[ei];
            if !matches!(e.kind, EdgeKind::Calls | EdgeKind::Requests) {
                continue;
            }
            let Some(t) = self.idx(&e.to) else { continue };
            let target = &self.graph.nodes[t];
            let reaches_io = target
                .effects
                .transitive
                .iter()
                .any(|k| IO_KINDS.contains(k))
                || target.kind == NodeKind::Route;
            if !reaches_io {
                continue;
            }
            items.push(IoItem {
                label: target.name.clone(),
                line: e.line.unwrap_or(0),
                loop_depth: e.loop_depth,
                awaited: e.awaited,
                conditional: e.conditional,
                root: match e.attrs.get("receiver") {
                    Some(r) => first_segment(r),
                    None => module_of(&target.qualified_name),
                },
                is_method: e.attrs.contains_key("receiver"),
            });
        }
        items.sort_by_key(|it| (it.line, it.label.clone()));
        items.dedup_by_key(|it| it.line);
        items
    }

    fn io_in_loop(&self, i: usize, out: &mut Vec<Insight>) {
        let n = &self.graph.nodes[i];
        let mut seen: HashSet<(u32, String)> = HashSet::new();
        // Module-level loops register handlers rather than call them.
        let registration_code = n.name == "<module>";
        for e in &n.effects.direct {
            if e.loop_depth > 0
                && LOOP_IO_KINDS.contains(&e.kind)
                && seen.insert((e.line, e.via.clone()))
            {
                let target = e
                    .target
                    .as_deref()
                    .map(|t| format!(" ({t})"))
                    .unwrap_or_default();
                out.push(Insight::IoInLoop {
                    node: n.id.clone(),
                    effect: format!("{} via {}{target}", kind_name(e.kind), e.via),
                    line: e.line,
                    observed_calls_per_parent: None,
                });
            }
        }
        for &ei in &self.outgoing[i] {
            let e = &self.graph.edges[ei];
            if registration_code
                || e.loop_depth == 0
                || !matches!(e.kind, EdgeKind::Calls | EdgeKind::Spawns)
                || !plausible_call(e)
            {
                continue;
            }
            let Some(t) = self.idx(&e.to) else { continue };
            let target = &self.graph.nodes[t];
            if target.entrypoint.is_some_and(|k| {
                matches!(k, EntrypointKind::UiHandler | EntrypointKind::HttpHandler)
            }) {
                continue;
            }
            let kinds: Vec<&str> = target
                .effects
                .transitive
                .iter()
                .filter(|k| LOOP_IO_KINDS.contains(k))
                .map(|k| kind_name(*k))
                .collect();
            if kinds.is_empty() {
                continue;
            }
            let line = e.line.unwrap_or(0);
            if seen.insert((line, target.name.clone())) {
                out.push(Insight::IoInLoop {
                    node: n.id.clone(),
                    effect: format!("{} via {}", kinds.join("+"), target.name),
                    line,
                    observed_calls_per_parent: None,
                });
            }
        }
    }

    /// Two or more consecutive awaited I/O items at the same loop depth, none conditional,
    /// each on a distinct receiver root, no later item on a local receiver (probably derived
    /// from an earlier result, like `cur = await conn.execute(); cur.fetchall()`), no plain
    /// call on a line in between (which probably consumes the earlier result), no later item
    /// reading a name bound or mutably borrowed since the first, no ordering barrier
    /// (commit, lock, send, ...) in between, and no table written by one and touched by another.
    fn parallelisable(&self, i: usize, out: &mut Vec<Insight>) {
        let n = &self.graph.nodes[i];
        let boundaries: Vec<u32> = self.outgoing[i]
            .iter()
            .map(|&ei| &self.graph.edges[ei])
            .filter(|e| e.kind == EdgeKind::Calls && !e.awaited)
            .filter_map(|e| e.line)
            .collect();
        let params: HashSet<&str> = n
            .function
            .as_ref()
            .map(|f| f.params.iter().map(|p| p.name.as_str()).collect())
            .unwrap_or_default();
        let flows: &[Flow] = n.function.as_ref().map_or(&[], |f| f.flows.as_slice());
        let flow_at = |line: u32| flows.iter().find(|f| f.line == line);
        let tables = self.table_accesses(i);
        let ordered = |since: u32, it: &IoItem| {
            let (uses, mutates) = flow_at(it.line).map_or((&[][..], &[][..]), |f| {
                (f.uses.as_slice(), f.mutates.as_slice())
            });
            let shares = |a: &[String], b: &[String]| a.iter().any(|x| b.contains(x));
            let data = flows
                .iter()
                .filter(|f| since <= f.line && f.line < it.line)
                .any(|f| {
                    shares(&f.binds, uses) || shares(&f.mutates, uses) || shares(&f.uses, mutates)
                });
            let barrier = flows
                .iter()
                .filter(|f| since <= f.line && f.line <= it.line)
                .any(|f| f.barrier)
                || n.effects
                    .direct
                    .iter()
                    .any(|e| e.kind == EffectKind::Lock && since <= e.line && e.line <= it.line);
            let table = tables.iter().any(|a| {
                since <= a.line
                    && a.line < it.line
                    && tables
                        .iter()
                        .any(|b| b.line == it.line && b.table == a.table && (a.write || b.write))
            });
            data || barrier || table
        };
        let items = self.io_items(i);
        let mut run: Vec<&IoItem> = Vec::new();
        let mut flush = |run: &mut Vec<&IoItem>| {
            if run.len() >= 2 {
                out.push(Insight::Parallelisable {
                    node: n.id.clone(),
                    children: run
                        .iter()
                        .map(|it| format!("{}:{}", it.label, it.line))
                        .collect(),
                    saved_nanos: 0,
                });
            }
            run.clear();
        };
        for it in &items {
            let conditional = it.conditional || flow_at(it.line).is_some_and(|f| f.conditional);
            if !it.awaited || conditional {
                flush(&mut run);
                continue;
            }
            let consumed_between = run
                .last()
                .is_some_and(|prev| boundaries.iter().any(|&l| prev.line < l && l < it.line));
            let independent = run
                .iter()
                .all(|prev| prev.loop_depth == it.loop_depth && prev.root != it.root)
                && (run.is_empty() || !it.local_receiver(&params))
                && !consumed_between
                && !run.first().is_some_and(|first| ordered(first.line, it));
            if independent {
                run.push(it);
            } else {
                flush(&mut run);
                run.push(it);
            }
        }
        flush(&mut run);
    }

    /// Table accesses of a function: its own `reads`/`writes` edges plus those of the
    /// functions it calls, placed at the line of the call. Callees that take a lock or open a
    /// transaction themselves are left out.
    fn table_accesses(&self, i: usize) -> Vec<TableAccess> {
        let n = &self.graph.nodes[i];
        let deletes_at = |node: &Node, line: Option<u32>| {
            node.effects
                .direct
                .iter()
                .any(|e| e.kind == EffectKind::DbDelete && Some(e.line) == line)
        };
        let mut out = Vec::new();
        for &ei in &self.outgoing[i] {
            let e = &self.graph.edges[ei];
            let Some(line) = e.line else { continue };
            match e.kind {
                EdgeKind::Reads | EdgeKind::Writes => out.push(TableAccess {
                    table: e.to.trim_start_matches("table:").to_owned(),
                    line,
                    write: e.kind == EdgeKind::Writes,
                    delete: deletes_at(n, Some(line)),
                }),
                EdgeKind::Calls => {
                    let Some(t) = self.idx(&e.to) else { continue };
                    let callee = &self.graph.nodes[t];
                    if callee
                        .effects
                        .direct
                        .iter()
                        .any(|d| d.kind == EffectKind::Lock)
                    {
                        continue;
                    }
                    for &ci in &self.outgoing[t] {
                        let c = &self.graph.edges[ci];
                        if !matches!(c.kind, EdgeKind::Reads | EdgeKind::Writes) {
                            continue;
                        }
                        out.push(TableAccess {
                            table: c.to.trim_start_matches("table:").to_owned(),
                            line,
                            write: c.kind == EdgeKind::Writes,
                            delete: deletes_at(callee, c.line),
                        });
                    }
                }
                _ => {}
            }
        }
        out.sort_by(|a, b| (a.line, &a.table, a.write).cmp(&(b.line, &b.table, b.write)));
        out
    }

    /// A read of a table followed by a write to it, with an awaited call or effect from the
    /// read line up to the write line, and no lock or transaction taken before the write.
    /// Functions handed a transaction by their caller (`tx: &mut Transaction<'_, _>`) run
    /// inside it and are skipped.
    fn read_then_write(&self, i: usize, out: &mut Vec<Insight>) {
        let n = &self.graph.nodes[i];
        if self.is_test(i)
            || n.function
                .as_ref()
                .is_some_and(|f| in_transaction(&f.params))
        {
            return;
        }
        let accesses = self.table_accesses(i);
        if !accesses.iter().any(|a| a.write) || !accesses.iter().any(|a| !a.write) {
            return;
        }
        let lock_before = n
            .effects
            .direct
            .iter()
            .filter(|e| e.kind == EffectKind::Lock)
            .map(|e| e.line)
            .min();
        let mut awaits: Vec<u32> = n
            .effects
            .direct
            .iter()
            .filter(|e| e.awaited)
            .map(|e| e.line)
            .collect();
        awaits.extend(
            self.outgoing[i]
                .iter()
                .map(|&ei| &self.graph.edges[ei])
                .filter(|e| e.awaited)
                .filter_map(|e| e.line),
        );
        let mut reported: HashSet<&str> = HashSet::new();
        for r in accesses.iter().filter(|a| !a.write) {
            if reported.contains(r.table.as_str()) {
                continue;
            }
            let Some(w) = accesses
                .iter()
                .find(|a| a.write && a.table == r.table && a.line > r.line)
            else {
                continue;
            };
            let yields = awaits.iter().any(|&l| l >= r.line && l < w.line);
            let guarded = lock_before.is_some_and(|l| l <= w.line);
            if yields && !guarded {
                reported.insert(r.table.as_str());
                out.push(Insight::ReadThenWrite {
                    node: n.id.clone(),
                    table: r.table.clone(),
                    read_line: r.line,
                    write_line: w.line,
                    write: if w.delete { "deletes" } else { "writes" }.to_owned(),
                });
            }
        }
    }

    fn blocking(&self, i: usize, info: &hoshizora_model::FunctionInfo, out: &mut Vec<Insight>) {
        let n = &self.graph.nodes[i];
        let mut reasons: Vec<String> = n
            .effects
            .direct
            .iter()
            .filter(|e| e.kind == EffectKind::Blocking)
            .map(|e| {
                format!(
                    "blocking call {} at line {} in async function",
                    e.via, e.line
                )
            })
            .collect();
        let js = matches!(
            n.language,
            Some(hoshizora_model::Language::TypeScript | hoshizora_model::Language::JavaScript)
        );
        if info.is_async && !js {
            for &ei in &self.outgoing[i] {
                let e = &self.graph.edges[ei];
                if e.kind != EdgeKind::Calls || !plausible_call(e) {
                    continue;
                }
                let Some(t) = self.idx(&e.to) else { continue };
                let target = &self.graph.nodes[t];
                let sync_callee = target.function.as_ref().is_some_and(|f| !f.is_async);
                let blocks = target
                    .effects
                    .transitive
                    .iter()
                    .any(|k| matches!(k, EffectKind::Blocking | EffectKind::Sleep));
                if sync_callee && blocks {
                    reasons.push(format!(
                        "blocking call in async context via {} at line {}",
                        target.name,
                        e.line.unwrap_or(0)
                    ));
                }
            }
        }
        if !reasons.is_empty() {
            out.push(Insight::Inefficient {
                node: n.id.clone(),
                self_p95_nanos: None,
                complexity: info.complexity.time.clone(),
                reasons,
            });
        }
    }

    fn complexity_candidate(
        &self,
        n: &Node,
        info: &hoshizora_model::FunctionInfo,
        out: &mut Vec<Insight>,
    ) {
        let mut reasons = Vec::new();
        if info.max_loop_depth >= 2 {
            reasons.push(format!("nested loops (depth {})", info.max_loop_depth));
        }
        if info.cyclomatic >= 15 {
            reasons.push(format!("cyclomatic complexity {}", info.cyclomatic));
        }
        if info.complexity.time == "O(2^n)" || info.complexity.time.starts_with("O(n^") {
            reasons.push(format!("estimated {}", info.complexity.time));
        }
        if reasons.len() >= 2 || (reasons.len() == 1 && info.complexity.time == "O(2^n)") {
            reasons.extend(info.complexity.basis.iter().cloned());
            out.push(Insight::Inefficient {
                node: n.id.clone(),
                self_p95_nanos: None,
                complexity: info.complexity.time.clone(),
                reasons,
            });
        }
    }
}

const IO_KINDS: &[EffectKind] = &[
    EffectKind::Network,
    EffectKind::DbRead,
    EffectKind::DbWrite,
    EffectKind::DbDelete,
    EffectKind::FsRead,
    EffectKind::FsWrite,
];

const LOOP_IO_KINDS: &[EffectKind] = &[
    EffectKind::Network,
    EffectKind::DbRead,
    EffectKind::DbWrite,
    EffectKind::DbDelete,
];

fn kind_name(kind: EffectKind) -> &'static str {
    match kind {
        EffectKind::Network => "network",
        EffectKind::DbRead => "db_read",
        EffectKind::DbWrite => "db_write",
        EffectKind::DbDelete => "db_delete",
        EffectKind::FsRead => "fs_read",
        EffectKind::FsWrite => "fs_write",
        EffectKind::Process => "process",
        EffectKind::Spawn => "spawn",
        EffectKind::Join => "join",
        EffectKind::Lock => "lock",
        EffectKind::Publish => "publish",
        EffectKind::Subscribe => "subscribe",
        EffectKind::Sleep => "sleep",
        EffectKind::Blocking => "blocking",
    }
}

fn in_transaction(params: &[hoshizora_model::Param]) -> bool {
    params.iter().any(|p| {
        matches!(p.name.as_str(), "tx" | "txn" | "trx" | "transaction")
            || p.ty.as_deref().is_some_and(|t| {
                t.contains("Transaction") || t.contains("Tx<") || t.ends_with("Tx")
            })
    })
}

struct TableAccess {
    table: String,
    line: u32,
    write: bool,
    delete: bool,
}

struct IoItem {
    label: String,
    line: u32,
    loop_depth: u32,
    awaited: bool,
    conditional: bool,
    /// Receiver or module the call goes through: `conn` for `conn.execute`, `crate::db` for
    /// `db::inventory`, the function name for a bare call.
    root: String,
    is_method: bool,
}

impl IoItem {
    /// A receiver that is neither a parameter nor `self` is a local, probably built from an
    /// earlier awaited result; a computed receiver (`a().await?.b()`) certainly is.
    fn local_receiver(&self, params: &HashSet<&str>) -> bool {
        self.is_method
            && !matches!(self.root.as_str(), "self" | "this" | "cls")
            && !params.contains(self.root.as_str())
            && (self.root == "<expr>" || self.root.chars().next().is_some_and(|c| c.is_lowercase()))
    }
}

fn first_segment(s: &str) -> String {
    s.split(['.', ':'])
        .find(|p| !p.is_empty())
        .unwrap_or(s)
        .to_owned()
}

fn module_of(qualified: &str) -> String {
    qualified
        .rsplit_once("::")
        .or_else(|| qualified.rsplit_once('.'))
        .map(|(m, _)| m.to_owned())
        .unwrap_or_else(|| qualified.to_owned())
}

fn effect_root(via: &str) -> String {
    if via.contains("::") {
        module_of(via)
    } else {
        first_segment(via)
    }
}

fn fold_name(s: &str) -> String {
    s.chars()
        .filter(|c| !matches!(c, '_' | '-' | ' '))
        .flat_map(char::to_lowercase)
        .collect()
}

/// Lowercase words of a name, split on separators and camelCase boundaries.
fn name_tokens(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for c in s.chars() {
        let boundary = matches!(c, '_' | '-' | ' ' | '.' | ':' | '/') || c.is_uppercase();
        if boundary && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
        if c.is_alphanumeric() {
            cur.extend(c.to_lowercase());
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn is_subsequence(needle: &str, hay: &str) -> bool {
    let mut it = hay.chars();
    needle.chars().all(|c| it.any(|h| h == c))
}

/// Swimlane for the flow panel: the package name for code nodes, the node kind otherwise.
fn lane_of(graph: &Graph, by_id: &HashMap<String, usize>, i: usize) -> String {
    let mut cur = i;
    for _ in 0..8 {
        let n = &graph.nodes[cur];
        if n.kind == NodeKind::Package {
            return n.name.clone();
        }
        match n.parent.as_ref().and_then(|p| by_id.get(p)) {
            Some(&p) => cur = p,
            None => break,
        }
    }
    let n = &graph.nodes[i];
    match n.kind {
        NodeKind::Route => "routes".to_owned(),
        NodeKind::Table => "tables".to_owned(),
        NodeKind::Host => "hosts".to_owned(),
        NodeKind::Topic => "topics".to_owned(),
        NodeKind::Dependency => "dependencies".to_owned(),
        _ => n
            .language
            .map(|l| format!("{l:?}").to_lowercase())
            .unwrap_or_else(|| "unknown".to_owned()),
    }
}
