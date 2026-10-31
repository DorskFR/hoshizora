//! Condensed, human-readable flows.

use std::collections::{HashMap, HashSet, VecDeque};

use hoshizora_model::api::{LaneKind, Story, StoryLane, StoryLink, StoryStep};
use hoshizora_model::{
    Confidence, Edge, EdgeKind, EffectKind, EntrypointKind, Language, Node, NodeKind,
};

use crate::Index;
use crate::index::DEFAULT_FLOW_KINDS;

pub const STORY_STEP_LIMIT: usize = 60;

/// Walk position: the raw node, the nearest kept step it belongs to, and whether the path
/// from that step was fire-and-forget at some point.
struct Visit {
    node: usize,
    depth: u32,
    step: usize,
    detached: bool,
}

impl Index {
    /// What `from` does, as a short sequence of steps grouped by process. Breadth-first, so
    /// the steps closest to the root survive the cap.
    pub fn story(&self, from: &str, depth: u32) -> Option<Story> {
        let root = self.idx(from)?;
        let mut steps: Vec<StoryStep> = vec![self.step(root, 0)];
        let mut links: Vec<StoryLink> = Vec::new();
        let mut step_of: HashMap<usize, usize> = HashMap::from([(root, 0)]);
        let mut visited: HashSet<usize> = HashSet::from([root]);
        let mut queue = VecDeque::from([Visit {
            node: root,
            depth: 0,
            step: 0,
            detached: false,
        }]);
        let mut truncated = false;
        while let Some(v) = queue.pop_front() {
            if v.depth >= depth {
                continue;
            }
            for (e, t) in self.story_edges(v.node) {
                let edge = &self.graph.edges[e];
                let detached = v.detached || is_detached(edge, &self.graph.nodes[t]);
                if let Some(&existing) = step_of.get(&t) {
                    if existing != v.step {
                        push_link(
                            &mut links,
                            self.link(v.step, existing, edge, detached, &steps),
                        );
                    }
                    continue;
                }
                if !visited.insert(t) {
                    continue;
                }
                if self.keeps(t, edge.kind) {
                    if steps.len() >= STORY_STEP_LIMIT {
                        truncated = true;
                        continue;
                    }
                    let seq = steps.len() as u32;
                    steps.push(self.step(t, seq));
                    let si = steps.len() - 1;
                    step_of.insert(t, si);
                    push_link(&mut links, self.link(v.step, si, edge, detached, &steps));
                    queue.push_back(Visit {
                        node: t,
                        depth: v.depth + 1,
                        step: si,
                        detached: false,
                    });
                } else {
                    let step = &mut steps[v.step];
                    step.folded.push(self.graph.nodes[t].id.clone());
                    step.effects
                        .extend(self.graph.nodes[t].effects.direct.iter().cloned());
                    queue.push_back(Visit {
                        node: t,
                        depth: v.depth + 1,
                        step: v.step,
                        detached,
                    });
                }
            }
        }
        let lanes = self.story_lanes(&steps);
        Some(Story {
            root: from.to_string(),
            lanes,
            steps,
            links,
            truncated,
        })
    }

    /// Flow edges worth following with their target, in source order: never into tests,
    /// never along a name match that could mean several things. A topic continues into its
    /// subscribers.
    fn story_edges(&self, i: usize) -> Vec<(usize, usize)> {
        if self.graph.nodes[i].kind == NodeKind::Topic {
            let mut out: Vec<(usize, usize)> = self.incoming[i]
                .iter()
                .copied()
                .filter(|&e| self.graph.edges[e].kind == EdgeKind::Subscribes)
                .filter_map(|e| self.idx(&self.graph.edges[e].from).map(|t| (e, t)))
                .filter(|&(_, t)| !self.is_test(t))
                .collect();
            out.sort_by_key(|&(_, t)| self.graph.nodes[t].id.clone());
            return out;
        }
        let mut by_site: HashMap<(u32, EdgeKind, &str), Vec<usize>> = HashMap::new();
        let mut order = Vec::new();
        for &e in &self.outgoing[i] {
            let edge = &self.graph.edges[e];
            if !DEFAULT_FLOW_KINDS.contains(&edge.kind) || edge.kind == EdgeKind::Subscribes {
                continue;
            }
            let Some(t) = self.idx(&edge.to) else {
                continue;
            };
            if self.is_test(t) {
                continue;
            }
            let key = (
                edge.line.unwrap_or(0),
                edge.kind,
                self.graph.nodes[t].name.as_str(),
            );
            let bucket = by_site.entry(key).or_default();
            if bucket.is_empty() {
                order.push(key);
            }
            bucket.push(e);
        }
        let mut out = Vec::new();
        for key in order {
            let candidates = &by_site[&key];
            if let Some(e) = self.pick(i, candidates)
                && let Some(t) = self.idx(&self.graph.edges[e].to)
            {
                out.push((e, t));
            }
        }
        out.sort_by_key(|&(e, _)| (self.graph.edges[e].line.unwrap_or(0), e));
        out
    }

    /// One edge among those written at the same call site, or none when the name match is
    /// ambiguous: exact and observed edges win; a name match counts only when it is the
    /// sole candidate, or the sole one in the caller's file, then package.
    fn pick(&self, from: usize, candidates: &[usize]) -> Option<usize> {
        let sure: Vec<usize> = candidates
            .iter()
            .copied()
            .filter(|&e| {
                let edge = &self.graph.edges[e];
                match edge.confidence {
                    Confidence::Exact | Confidence::Observed => true,
                    Confidence::Heuristic => edge.kind != EdgeKind::Calls,
                    Confidence::Name => false,
                }
            })
            .collect();
        if let Some(&e) = sure.first() {
            return Some(e);
        }
        let caller = &self.graph.nodes[from];
        // A method name matched without a type (`x.error(..)`) is only trusted within the file.
        let candidates: Vec<usize> = candidates
            .iter()
            .copied()
            .filter(|&e| {
                let edge = &self.graph.edges[e];
                !edge.attrs.contains_key("receiver")
                    || self
                        .idx(&edge.to)
                        .is_some_and(|t| self.graph.nodes[t].file == caller.file)
            })
            .collect();
        if candidates.len() == 1 {
            return candidates.first().copied();
        }
        let same_file: Vec<usize> = candidates
            .iter()
            .copied()
            .filter(|&e| {
                self.idx(&self.graph.edges[e].to)
                    .is_some_and(|t| self.graph.nodes[t].file == caller.file)
            })
            .collect();
        if same_file.len() == 1 {
            return same_file.first().copied();
        }
        let same_lane: Vec<usize> = candidates
            .iter()
            .copied()
            .filter(|&e| {
                self.idx(&self.graph.edges[e].to)
                    .is_some_and(|t| self.lanes[t] == self.lanes[from])
            })
            .collect();
        if same_lane.len() == 1 {
            return same_lane.first().copied();
        }
        None
    }

    pub(crate) fn is_test(&self, i: usize) -> bool {
        let n = &self.graph.nodes[i];
        n.entrypoint == Some(EntrypointKind::Test)
            || n.file.as_deref().is_some_and(is_test_file)
            || n.qualified_name
                .split("::")
                .any(|seg| seg == "tests" || seg == "test")
    }

    /// A node becomes its own step when it is not a helper: synthetic nodes, route handlers,
    /// spawned work, and functions that talk to something other than the database. Helpers
    /// that only touch tables fold into their caller, which then reads as "writes sessions";
    /// a call into another package is a library call, not a process boundary, so it folds
    /// the same way (routes, topics and hosts are the boundaries).
    fn keeps(&self, i: usize, via: EdgeKind) -> bool {
        let n = &self.graph.nodes[i];
        match n.kind {
            NodeKind::Route | NodeKind::Table | NodeKind::Host | NodeKind::Topic => true,
            NodeKind::Function => {
                matches!(via, EdgeKind::Handles | EdgeKind::Spawns)
                    || n.effects
                        .direct
                        .iter()
                        .any(|e| is_story_effect(e.kind) && !is_db(e.kind))
            }
            _ => false,
        }
    }

    fn step(&self, i: usize, seq: u32) -> StoryStep {
        let n = &self.graph.nodes[i];
        StoryStep {
            node: n.id.clone(),
            lane: self.lane_id(i),
            label: step_label(n),
            kind: n.kind,
            seq,
            effects: n.effects.direct.clone(),
            folded: Vec::new(),
        }
    }

    fn link(
        &self,
        from: usize,
        to: usize,
        edge: &Edge,
        detached: bool,
        steps: &[StoryStep],
    ) -> StoryLink {
        let named = if edge.kind == EdgeKind::Handles {
            self.node(&edge.from)
        } else {
            self.node(&edge.to)
        };
        StoryLink {
            from: steps[from].node.clone(),
            to: steps[to].node.clone(),
            kind: edge.kind,
            label: link_label(edge.kind, named),
            seq: steps[to].seq,
            detached,
            confidence: edge.confidence,
        }
    }

    fn package_of(&self, i: usize) -> Option<usize> {
        let mut cur = i;
        for _ in 0..8 {
            let n = &self.graph.nodes[cur];
            if n.kind == NodeKind::Package {
                return Some(cur);
            }
            cur = self.idx(n.parent.as_deref()?)?;
        }
        None
    }

    fn lane_id(&self, i: usize) -> String {
        match self.graph.nodes[i].kind {
            NodeKind::Table => "database".to_owned(),
            NodeKind::Topic => "queue".to_owned(),
            NodeKind::Host => "external".to_owned(),
            NodeKind::Route => self
                .route_package(i)
                .map(|p| self.graph.nodes[p].id.clone())
                .unwrap_or_else(|| "routes".to_owned()),
            _ => self
                .package_of(i)
                .map(|p| self.graph.nodes[p].id.clone())
                .unwrap_or_else(|| self.lanes[i].clone()),
        }
    }

    /// Package of the function handling a route, so the route sits in the service's lane.
    fn route_package(&self, i: usize) -> Option<usize> {
        self.outgoing[i]
            .iter()
            .map(|&e| &self.graph.edges[e])
            .filter(|e| e.kind == EdgeKind::Handles)
            .filter_map(|e| self.idx(&e.to))
            .find_map(|t| self.package_of(t))
    }

    fn lane_kind(&self, id: &str) -> LaneKind {
        match id {
            "database" => return LaneKind::Database,
            "queue" => return LaneKind::Queue,
            "external" => return LaneKind::External,
            _ => {}
        }
        let Some(p) = self.idx(id) else {
            return LaneKind::Worker;
        };
        let pkg = &self.graph.nodes[p];
        if matches!(
            pkg.language,
            Some(Language::TypeScript | Language::JavaScript)
        ) {
            return LaneKind::Ui;
        }
        let serves = self.graph.edges.iter().any(|e| {
            e.kind == EdgeKind::Handles
                && self
                    .idx(&e.to)
                    .and_then(|t| self.package_of(t))
                    .is_some_and(|tp| tp == p)
        });
        if serves {
            LaneKind::Service
        } else {
            LaneKind::Worker
        }
    }

    fn story_lanes(&self, steps: &[StoryStep]) -> Vec<StoryLane> {
        let mut seen = HashSet::new();
        let mut lanes: Vec<StoryLane> = steps
            .iter()
            .filter(|s| seen.insert(s.lane.clone()))
            .map(|s| {
                let kind = self.lane_kind(&s.lane);
                let label = match kind {
                    LaneKind::Database => "Database".to_owned(),
                    LaneKind::Queue => "Queue".to_owned(),
                    LaneKind::External => "External".to_owned(),
                    _ => self
                        .node(&s.lane)
                        .map(|n| n.name.clone())
                        .unwrap_or_else(|| s.lane.clone()),
                };
                StoryLane {
                    id: s.lane.clone(),
                    label,
                    kind,
                }
            })
            .collect();
        lanes.sort_by_key(|l| {
            matches!(
                l.kind,
                LaneKind::Database | LaneKind::Queue | LaneKind::External
            )
        });
        lanes
    }
}

/// Effects that mean something to a reader of the flow; locks, joins and sleeps do not.
fn is_story_effect(kind: EffectKind) -> bool {
    !matches!(
        kind,
        EffectKind::Lock | EffectKind::Join | EffectKind::Sleep | EffectKind::Blocking
    )
}

fn is_db(kind: EffectKind) -> bool {
    matches!(kind, EffectKind::DbRead | EffectKind::DbWrite)
}

fn push_link(links: &mut Vec<StoryLink>, link: StoryLink) {
    if !links
        .iter()
        .any(|l| l.from == link.from && l.to == link.to && l.kind == link.kind)
    {
        links.push(link);
    }
}

fn is_detached(edge: &Edge, target: &Node) -> bool {
    edge.kind == EdgeKind::Spawns
        || (edge.kind == EdgeKind::Calls
            && !edge.awaited
            && target.function.as_ref().is_some_and(|f| f.is_async))
}

fn step_label(n: &Node) -> String {
    match n.kind {
        NodeKind::Function => match n.parent.as_deref().and_then(|p| p.rsplit_once('#')) {
            Some((prefix, owner)) if prefix.starts_with("type:") => {
                format!("{owner}.{}", n.name)
            }
            _ => n.name.clone(),
        },
        _ => n.name.clone(),
    }
}

fn link_label(kind: EdgeKind, named: Option<&Node>) -> Option<String> {
    let name = named.map(|n| n.name.as_str()).unwrap_or("?");
    match kind {
        EdgeKind::Requests | EdgeKind::Handles => Some(name.to_owned()),
        EdgeKind::Reads => Some(format!("read {name}")),
        EdgeKind::Writes => Some(format!("write {name}")),
        EdgeKind::Publishes => Some(format!("publish {name}")),
        EdgeKind::Subscribes => Some(format!("subscribe {name}")),
        EdgeKind::Spawns => Some("spawn".to_owned()),
        _ => None,
    }
}

pub(crate) fn is_test_file(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.starts_with("test_")
        || name.ends_with("_test.py")
        || name.ends_with("_test.go")
        || name.ends_with(".test.ts")
        || name.ends_with(".test.tsx")
        || name.ends_with(".test.js")
        || name.ends_with(".spec.ts")
        || name.ends_with(".spec.js")
        || path
            .split('/')
            .any(|seg| matches!(seg, "tests" | "test" | "__tests__" | "e2e" | "journeys"))
}
