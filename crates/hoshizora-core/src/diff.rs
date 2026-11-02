//! What changed between two graphs of the same repo: functions, entrypoint effects, tables,
//! insights and impact. Nothing here compares line numbers, which shift with every edit.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::path::Path;

use hoshizora_model::{EffectKind, EntrypointKind, Insight, Language, Node, NodeKind, StateSite};
use serde::{Deserialize, Serialize};

use crate::index::{DEFAULT_FLOW_KINDS, Index};

pub const IMPACT_CAP: usize = 10;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiffReport {
    pub functions: FunctionDelta,
    pub entrypoint_effect_delta: Vec<EffectDelta>,
    pub tests_effect_delta: Vec<EffectDelta>,
    pub tables_touched: BTreeMap<String, TableContext>,
    pub insights: InsightDelta,
    pub impact: BTreeMap<String, ImpactSummary>,
    #[serde(default)]
    pub states: StatesDelta,
}

/// Value domains whose members changed, and the branch sites that do not follow.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct StatesDelta {
    pub domains: Vec<DomainDelta>,
    /// Sites in added or modified functions with missing values and a catch-all that swallows
    /// them (same rule as the `unhandled_values` insight).
    pub silent_defaults: Vec<StateSite>,
}

/// Domains are matched by name within a language family; same-named domains are merged.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DomainDelta {
    pub domain: String,
    pub language: Language,
    pub added: Vec<String>,
    pub removed: Vec<String>,
    /// Every site in head linked to the domain that handles none of the added values it
    /// lists, anywhere in the repo.
    pub unhandled: Vec<UnhandledSite>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnhandledSite {
    #[serde(flatten)]
    pub site: StateSite,
    /// Added values this site has no branch for.
    pub values: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct FunctionDelta {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    /// Same id in both graphs, different body text.
    pub modified: Vec<String>,
    pub moved: Vec<Moved>,
}

/// A removed and an added function with the same body, or the same name when that pairing
/// is unambiguous; `modified` is set for the latter when the body changed too.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Moved {
    pub from: String,
    pub to: String,
    pub modified: bool,
}

/// Effects reachable from an entrypoint, as `kind` or `kind:target`, before and after.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EffectDelta {
    pub entrypoint: String,
    pub added: Vec<String>,
    pub removed: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct TableContext {
    pub db_read: Vec<String>,
    pub db_write: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct InsightDelta {
    pub new: Vec<Insight>,
    pub resolved: Vec<Insight>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ImpactSummary {
    pub entrypoints: Vec<ImpactEntry>,
    /// User-facing entrypoints past the cap; tests and library exports are never counted.
    pub omitted: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImpactEntry {
    pub node: String,
    pub kind: String,
    /// Shortest flow path from the entrypoint to the changed function.
    pub path: Vec<String>,
}

pub fn diff(base: &Index, head: &Index) -> DiffReport {
    let mut base_src = Sources::new(&base.graph.meta.root);
    let mut head_src = Sources::new(&head.graph.meta.root);
    let base_fns: BTreeMap<&str, &Node> = functions(base);
    let head_fns: BTreeMap<&str, &Node> = functions(head);

    let mut added: Vec<&str> = head_fns
        .keys()
        .copied()
        .filter(|id| !base_fns.contains_key(id))
        .collect();
    let mut removed: Vec<&str> = base_fns
        .keys()
        .copied()
        .filter(|id| !head_fns.contains_key(id))
        .collect();
    let mut modified: Vec<String> = Vec::new();
    for (id, h) in &head_fns {
        let Some(b) = base_fns.get(id) else { continue };
        if !same_body(base_src.body(b), head_src.body(h), b, h) {
            modified.push((*id).to_owned());
        }
    }

    let moved = pair_moved(
        &mut removed,
        &mut added,
        &base_fns,
        &head_fns,
        &mut base_src,
        &mut head_src,
    );
    let moved_from_to: HashMap<&str, &str> = moved
        .iter()
        .map(|m| (m.from.as_str(), m.to.as_str()))
        .collect();
    let moved_to_from: HashMap<&str, &str> = moved
        .iter()
        .map(|m| (m.to.as_str(), m.from.as_str()))
        .collect();

    let mut changed: Vec<String> = added.iter().map(|s| (*s).to_owned()).collect();
    changed.extend(modified.iter().cloned());
    changed.extend(moved.iter().filter(|m| m.modified).map(|m| m.to.clone()));
    changed.sort();
    changed.dedup();

    let (entrypoint_effect_delta, tests_effect_delta) =
        effect_deltas(base, head, &moved_from_to, &moved_to_from);
    let tables_touched = tables(head, &head_fns, &changed);
    let insights = insight_delta(base, head, &moved_from_to);
    let impact = changed
        .iter()
        .map(|id| (id.clone(), impact_of(head, id)))
        .collect();
    let states = states_delta(base, head, &changed);

    DiffReport {
        functions: FunctionDelta {
            added: added.into_iter().map(str::to_owned).collect(),
            removed: removed.into_iter().map(str::to_owned).collect(),
            modified,
            moved,
        },
        entrypoint_effect_delta,
        tests_effect_delta,
        tables_touched,
        insights,
        impact,
        states,
    }
}

fn family(language: Language) -> &'static str {
    match language {
        Language::TypeScript | Language::JavaScript => "typescript",
        Language::Rust => "rust",
        Language::Python => "python",
        Language::Go => "go",
    }
}

/// Member names across every same-named domain, and the ids merged.
type MergedDomain<'a> = (Language, BTreeSet<&'a str>, Vec<&'a str>);

/// `(family, name)` → merged domain.
fn domain_values(index: &Index) -> BTreeMap<(&str, &str), MergedDomain<'_>> {
    let mut out: BTreeMap<(&str, &str), MergedDomain<'_>> = BTreeMap::new();
    for d in &index.graph.states.domains {
        let entry = out
            .entry((family(d.language), d.name.as_str()))
            .or_insert_with(|| (d.language, BTreeSet::new(), Vec::new()));
        entry.1.extend(d.values.iter().map(|v| v.name.as_str()));
        entry.2.push(d.id.as_str());
    }
    out
}

fn states_delta(base: &Index, head: &Index, changed: &[String]) -> StatesDelta {
    let before = domain_values(base);
    let after = domain_values(head);
    let empty = BTreeSet::new();
    let mut domains = Vec::new();
    for (key, (language, values, ids)) in &after {
        let old = before.get(key).map_or(&empty, |(_, v, _)| v);
        let added: Vec<String> = values.difference(old).map(|v| (*v).to_owned()).collect();
        let removed: Vec<String> = old.difference(values).map(|v| (*v).to_owned()).collect();
        if added.is_empty() && removed.is_empty() {
            continue;
        }
        let literal_of = |name: &str| -> Vec<&str> {
            head.graph
                .states
                .domains
                .iter()
                .filter(|d| ids.contains(&d.id.as_str()))
                .flat_map(|d| d.values.iter())
                .filter(|v| v.name == name)
                .filter_map(|v| v.literal.as_deref())
                .collect()
        };
        let unhandled = head
            .graph
            .states
            .sites
            .iter()
            .filter(|s| ids.contains(&s.domain.as_str()))
            .filter(|s| !head.idx(&s.function).is_some_and(|i| head.is_test(i)))
            .filter_map(|s| {
                let values: Vec<String> = added
                    .iter()
                    .filter(|a| {
                        !s.handled
                            .iter()
                            .any(|h| h == *a || literal_of(a).contains(&h.as_str()))
                    })
                    .cloned()
                    .collect();
                (!values.is_empty()).then(|| UnhandledSite {
                    site: s.clone(),
                    values,
                })
            })
            .collect();
        domains.push(DomainDelta {
            domain: key.1.to_owned(),
            language: *language,
            added,
            removed,
            unhandled,
        });
    }
    for (key, (language, values, _)) in &before {
        if after.contains_key(key) {
            continue;
        }
        domains.push(DomainDelta {
            domain: key.1.to_owned(),
            language: *language,
            added: Vec::new(),
            removed: values.iter().map(|v| (*v).to_owned()).collect(),
            unhandled: Vec::new(),
        });
    }
    domains.sort_by(|a, b| (family(a.language), &a.domain).cmp(&(family(b.language), &b.domain)));
    let silent_defaults = head
        .graph
        .states
        .sites
        .iter()
        .filter(|s| changed.binary_search(&s.function).is_ok() && head.silent_site(s))
        .cloned()
        .collect();
    StatesDelta {
        domains,
        silent_defaults,
    }
}

fn functions(index: &Index) -> BTreeMap<&str, &Node> {
    index
        .graph
        .nodes
        .iter()
        .filter(|n| n.kind == NodeKind::Function)
        .map(|n| (n.id.as_str(), n))
        .collect()
}

/// Source lines per file, read lazily from the graph's recorded root.
struct Sources<'a> {
    root: &'a Path,
    files: HashMap<String, Option<Vec<String>>>,
}

impl<'a> Sources<'a> {
    fn new(root: &'a str) -> Self {
        Self {
            root: Path::new(root),
            files: HashMap::new(),
        }
    }

    fn body(&mut self, n: &Node) -> Option<String> {
        let file = n.file.as_deref()?;
        let span = n.span?;
        let lines = self
            .files
            .entry(file.to_owned())
            .or_insert_with(|| {
                std::fs::read_to_string(self.root.join(file))
                    .ok()
                    .map(|s| s.lines().map(str::to_owned).collect())
            })
            .as_ref()?;
        let start = (span.start_line.max(1) - 1) as usize;
        let end = (span.end_line as usize).min(lines.len());
        (start < end).then(|| lines[start..end].join("\n"))
    }
}

/// Bodies compared as text; without sources (graph files whose root is gone) the signature,
/// length and direct effects stand in.
fn same_body(a: Option<String>, b: Option<String>, an: &Node, bn: &Node) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a == b,
        _ => fingerprint(an) == fingerprint(bn),
    }
}

fn fingerprint(n: &Node) -> String {
    let effects: Vec<String> = n
        .effects
        .direct
        .iter()
        .map(|e| format!("{}|{}", effect_label(e.kind, e.target.as_deref()), e.via))
        .collect();
    format!(
        "{}|{}|{}",
        n.function.as_ref().map_or("", |f| f.signature.as_str()),
        n.lines,
        effects.join(";")
    )
}

fn normalise(body: &str) -> String {
    body.chars().filter(|c| !c.is_whitespace()).collect()
}

fn pair_moved(
    removed: &mut Vec<&str>,
    added: &mut Vec<&str>,
    base_fns: &BTreeMap<&str, &Node>,
    head_fns: &BTreeMap<&str, &Node>,
    base_src: &mut Sources,
    head_src: &mut Sources,
) -> Vec<Moved> {
    let mut moved = Vec::new();
    let mut by_body: HashMap<String, Vec<&str>> = HashMap::new();
    for id in added.iter() {
        if let Some(b) = head_src.body(head_fns[id]) {
            by_body.entry(normalise(&b)).or_default().push(id);
        }
    }
    let mut taken: HashSet<&str> = HashSet::new();
    let mut still_removed = Vec::new();
    for id in removed.iter() {
        let candidates = base_src
            .body(base_fns[id])
            .and_then(|b| by_body.get(&normalise(&b)))
            .map(|v| {
                v.iter()
                    .copied()
                    .filter(|c| !taken.contains(c))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let name = &base_fns[id].name;
        let pick = match candidates.as_slice() {
            [] => None,
            [one] => Some(*one),
            many => many
                .iter()
                .copied()
                .find(|c| &head_fns[c].name == name)
                .or(Some(many[0])),
        };
        match pick {
            Some(to) => {
                taken.insert(to);
                moved.push(Moved {
                    from: (*id).to_owned(),
                    to: to.to_owned(),
                    modified: false,
                });
            }
            None => still_removed.push(*id),
        }
    }
    added.retain(|id| !taken.contains(id));
    *removed = still_removed;

    let mut added_by_name: HashMap<&str, Vec<&str>> = HashMap::new();
    for id in added.iter() {
        added_by_name
            .entry(head_fns[id].name.as_str())
            .or_default()
            .push(id);
    }
    let mut removed_by_name: HashMap<&str, Vec<&str>> = HashMap::new();
    for id in removed.iter() {
        removed_by_name
            .entry(base_fns[id].name.as_str())
            .or_default()
            .push(id);
    }
    let mut taken: HashSet<&str> = HashSet::new();
    let mut still_removed = Vec::new();
    for id in removed.iter() {
        let name = base_fns[id].name.as_str();
        let same_name = added_by_name.get(name).map(Vec::as_slice);
        match (removed_by_name[name].as_slice(), same_name) {
            ([_], Some(&[to])) => {
                taken.insert(to);
                moved.push(Moved {
                    from: (*id).to_owned(),
                    to: to.to_owned(),
                    modified: true,
                });
            }
            _ => still_removed.push(*id),
        }
    }
    added.retain(|id| !taken.contains(id));
    *removed = still_removed;
    moved.sort_by(|a, b| a.from.cmp(&b.from));
    moved
}

fn effect_label(kind: EffectKind, target: Option<&str>) -> String {
    let kind = serde_json::to_value(kind)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default();
    match target {
        Some(t) if !t.is_empty() => format!("{kind}:{t}"),
        _ => kind,
    }
}

/// Direct effects of everything reachable from `start` over flow edges.
fn reachable_effects(index: &Index, start: usize) -> BTreeSet<String> {
    let mut seen = vec![false; index.graph.nodes.len()];
    seen[start] = true;
    let mut stack = vec![start];
    let mut out = BTreeSet::new();
    while let Some(i) = stack.pop() {
        for e in &index.graph.nodes[i].effects.direct {
            out.insert(effect_label(e.kind, e.target.as_deref()));
        }
        for &ei in &index.outgoing[i] {
            let edge = &index.graph.edges[ei];
            if !DEFAULT_FLOW_KINDS.contains(&edge.kind) {
                continue;
            }
            if let Some(t) = index.idx(&edge.to)
                && !seen[t]
            {
                seen[t] = true;
                stack.push(t);
            }
        }
    }
    out
}

fn is_entrypoint(n: &Node) -> bool {
    n.entrypoint.is_some() || n.kind == NodeKind::Route
}

fn effect_deltas(
    base: &Index,
    head: &Index,
    moved_from_to: &HashMap<&str, &str>,
    moved_to_from: &HashMap<&str, &str>,
) -> (Vec<EffectDelta>, Vec<EffectDelta>) {
    let mut entrypoints = Vec::new();
    let mut tests = Vec::new();
    let mut push = |index: &Index, i: usize, delta: EffectDelta| {
        if index.is_test(i) {
            tests.push(delta);
        } else {
            entrypoints.push(delta);
        }
    };
    for (hi, n) in head.graph.nodes.iter().enumerate() {
        if !is_entrypoint(n) {
            continue;
        }
        let base_id = moved_to_from
            .get(n.id.as_str())
            .copied()
            .unwrap_or(n.id.as_str());
        let after = reachable_effects(head, hi);
        let before = base
            .idx(base_id)
            .map(|bi| reachable_effects(base, bi))
            .unwrap_or_default();
        if after != before {
            push(
                head,
                hi,
                EffectDelta {
                    entrypoint: n.id.clone(),
                    added: after.difference(&before).cloned().collect(),
                    removed: before.difference(&after).cloned().collect(),
                },
            );
        }
    }
    for (bi, n) in base.graph.nodes.iter().enumerate() {
        if !is_entrypoint(n)
            || head.idx(&n.id).is_some()
            || moved_from_to.contains_key(n.id.as_str())
        {
            continue;
        }
        let before = reachable_effects(base, bi);
        if !before.is_empty() {
            push(
                base,
                bi,
                EffectDelta {
                    entrypoint: n.id.clone(),
                    added: Vec::new(),
                    removed: before.into_iter().collect(),
                },
            );
        }
    }
    entrypoints.sort_by(|a, b| a.entrypoint.cmp(&b.entrypoint));
    tests.sort_by(|a, b| a.entrypoint.cmp(&b.entrypoint));
    (entrypoints, tests)
}

fn table_targets(n: &Node, kind: EffectKind) -> impl Iterator<Item = &str> {
    n.effects
        .direct
        .iter()
        .filter(move |e| e.kind == kind)
        .filter_map(|e| e.target.as_deref())
        .flat_map(|t| t.split(','))
        .map(str::trim)
        .filter(|t| !t.is_empty())
}

fn tables(
    head: &Index,
    head_fns: &BTreeMap<&str, &Node>,
    changed: &[String],
) -> BTreeMap<String, TableContext> {
    let mut out: BTreeMap<String, TableContext> = BTreeMap::new();
    for id in changed {
        let Some(n) = head_fns.get(id.as_str()) else {
            continue;
        };
        for kind in [EffectKind::DbRead, EffectKind::DbWrite] {
            for t in table_targets(n, kind) {
                out.entry(t.to_owned()).or_default();
            }
        }
    }
    for n in head
        .graph
        .nodes
        .iter()
        .filter(|n| n.kind == NodeKind::Function)
    {
        for t in table_targets(n, EffectKind::DbRead) {
            if let Some(ctx) = out.get_mut(t) {
                ctx.db_read.push(n.id.clone());
            }
        }
        for t in table_targets(n, EffectKind::DbWrite) {
            if let Some(ctx) = out.get_mut(t) {
                ctx.db_write.push(n.id.clone());
            }
        }
    }
    for ctx in out.values_mut() {
        ctx.db_read.sort();
        ctx.db_read.dedup();
        ctx.db_write.sort();
        ctx.db_write.dedup();
    }
    out
}

fn strip_line_suffix(s: &str) -> &str {
    match s.rsplit_once(':') {
        Some((head, tail)) if tail.chars().all(|c| c.is_ascii_digit()) => head,
        _ => s,
    }
}

fn strip_line_words(s: &str) -> String {
    let mut out = Vec::new();
    let mut words = s.split(' ').peekable();
    while let Some(w) = words.next() {
        if w == "line"
            && words
                .peek()
                .is_some_and(|n| n.chars().all(|c| c.is_ascii_digit()))
        {
            words.next();
            continue;
        }
        out.push(w);
    }
    out.join(" ")
}

/// Identity of an insight without its line numbers, with moved functions mapped to their
/// head id so a relocated finding is neither new nor resolved.
fn insight_key(i: &Insight, rename: &HashMap<&str, &str>) -> String {
    let node = |n: &str| rename.get(n).copied().unwrap_or(n).to_owned();
    match i {
        Insight::IoInLoop {
            node: n, effect, ..
        } => format!("io_in_loop|{}|{effect}", node(n)),
        Insight::Parallelisable {
            node: n, children, ..
        } => format!(
            "parallelisable|{}|{}",
            node(n),
            children
                .iter()
                .map(|c| strip_line_suffix(c))
                .collect::<Vec<_>>()
                .join(",")
        ),
        Insight::Inefficient {
            node: n,
            complexity,
            reasons,
            ..
        } => format!(
            "inefficient|{}|{complexity}|{}",
            node(n),
            reasons
                .iter()
                .map(|r| strip_line_words(r))
                .collect::<Vec<_>>()
                .join(";")
        ),
        Insight::HotPath { node: n, .. } => format!("hot_path|{}", node(n)),
        Insight::Waiting { node: n, on, .. } => format!("waiting|{}|{}", node(n), on.join(",")),
        Insight::NeverObserved { node: n } => format!("never_observed|{}", node(n)),
        Insight::UnhandledValues {
            node: n,
            domain,
            missing,
            ..
        } => format!(
            "unhandled_values|{}|{domain}|{}",
            node(n),
            missing.join(",")
        ),
    }
}

fn insight_delta(base: &Index, head: &Index, moved_from_to: &HashMap<&str, &str>) -> InsightDelta {
    let none = HashMap::new();
    let base_keys: HashMap<String, Insight> = base
        .static_insights()
        .into_iter()
        .map(|i| (insight_key(&i, moved_from_to), i))
        .collect();
    let head_keys: HashMap<String, Insight> = head
        .static_insights()
        .into_iter()
        .map(|i| (insight_key(&i, &none), i))
        .collect();
    let mut new: Vec<(&String, &Insight)> = head_keys
        .iter()
        .filter(|(k, _)| !base_keys.contains_key(*k))
        .collect();
    let mut resolved: Vec<(&String, &Insight)> = base_keys
        .iter()
        .filter(|(k, _)| !head_keys.contains_key(*k))
        .collect();
    new.sort_by(|a, b| a.0.cmp(b.0));
    resolved.sort_by(|a, b| a.0.cmp(b.0));
    InsightDelta {
        new: new.into_iter().map(|(_, i)| i.clone()).collect(),
        resolved: resolved.into_iter().map(|(_, i)| i.clone()).collect(),
    }
}

fn user_facing(index: &Index, i: usize) -> Option<String> {
    let n = &index.graph.nodes[i];
    if n.kind == NodeKind::Route {
        return Some("route".to_owned());
    }
    if index.is_test(i) {
        return None;
    }
    let kind = match n.entrypoint? {
        EntrypointKind::Main => "main",
        EntrypointKind::HttpHandler => "http_handler",
        EntrypointKind::UiHandler => "ui_handler",
        EntrypointKind::Cli => "cli",
        EntrypointKind::Job => "job",
        EntrypointKind::Test | EntrypointKind::Export => return None,
    };
    Some(kind.to_owned())
}

/// Breadth-first over incoming flow edges, so the first path found to each entrypoint is
/// the shortest; nearest entrypoints come first and the rest are counted.
fn impact_of(index: &Index, id: &str) -> ImpactSummary {
    let Some(target) = index.idx(id) else {
        return ImpactSummary::default();
    };
    let mut parent: HashMap<usize, usize> = HashMap::new();
    let mut seen = vec![false; index.graph.nodes.len()];
    seen[target] = true;
    let mut queue = VecDeque::from([target]);
    let mut hits: Vec<(usize, String)> = Vec::new();
    while let Some(i) = queue.pop_front() {
        for &ei in &index.incoming[i] {
            let edge = &index.graph.edges[ei];
            if !DEFAULT_FLOW_KINDS.contains(&edge.kind) {
                continue;
            }
            let Some(c) = index.idx(&edge.from) else {
                continue;
            };
            if seen[c] {
                continue;
            }
            seen[c] = true;
            parent.insert(c, i);
            queue.push_back(c);
            if let Some(kind) = user_facing(index, c) {
                hits.push((c, kind));
            }
        }
    }
    let omitted = hits.len().saturating_sub(IMPACT_CAP);
    hits.truncate(IMPACT_CAP);
    let entrypoints = hits
        .into_iter()
        .map(|(c, kind)| {
            let mut path = vec![index.graph.nodes[c].id.clone()];
            let mut cur = c;
            while let Some(&next) = parent.get(&cur) {
                path.push(index.graph.nodes[next].id.clone());
                cur = next;
            }
            ImpactEntry {
                node: index.graph.nodes[c].id.clone(),
                kind,
                path,
            }
        })
        .collect();
    ImpactSummary {
        entrypoints,
        omitted,
    }
}
