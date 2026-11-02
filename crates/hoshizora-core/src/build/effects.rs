//! Effect rules, route rules and the client → route / SQL → table / topic linkers.

use std::collections::{BTreeMap, HashMap, HashSet};

use hoshizora_model::{
    CallSite, Confidence, Edge, EdgeKind, Effect, EffectKind, Language, NodeKind,
};

use super::resolve::{callee_segments, resolve_call};
use super::{Ctx, SqlSite};
use crate::packs::{EffectRule, RouteStyle, RuleKind, Target};
use crate::pattern::Pattern;
use crate::sql;
use crate::url::{METHODS, method_from_name, normalise_path, parse as parse_url};

/// The callee as written plus the form with its first segment expanded to the import it binds
/// (`text` → `sqlalchemy.text`, `query` → `sqlx::query`).
fn expanded_callees(ctx: &Ctx, fi: usize, callee: &str) -> Vec<String> {
    let mut out = vec![callee.to_owned()];
    let segments = callee_segments(callee);
    let Some(first) = segments.first() else {
        return out;
    };
    let scope = &ctx.scopes[fi];
    if let Some(&imp) = scope.bindings.get(first) {
        let import = &scope.imports[imp];
        let sep = if ctx.files[fi].language == Language::Rust {
            "::"
        } else {
            "."
        };
        let path = import.path.trim_start_matches("node:");
        let base = match ctx.files[fi].imports.iter().find(|i| i.path == import.path) {
            Some(i) if i.names.contains(first) => format!("{path}{sep}{first}"),
            Some(i) if i.alias.as_deref() == Some(first) => path.to_owned(),
            _ => path.to_owned(),
        };
        let mut expanded = base;
        for seg in &segments[1..] {
            expanded.push_str(sep);
            expanded.push_str(seg);
        }
        if expanded != callee {
            out.push(expanded);
        }
    }
    out
}

/// Effect rules that can apply in one file, after the language and `requires_import` filters.
pub(crate) struct RuleSet<'r> {
    rules: Vec<&'r EffectRule>,
    exact: HashMap<&'r str, Vec<usize>>,
    wild: Vec<(&'r Pattern, usize)>,
}

impl<'r> RuleSet<'r> {
    fn new(rules: Vec<&'r EffectRule>) -> Self {
        let mut exact: HashMap<&'r str, Vec<usize>> = HashMap::new();
        let mut wild = Vec::new();
        for (i, rule) in rules.iter().enumerate() {
            for p in &rule.patterns {
                if p.source().contains('*') {
                    wild.push((p, i));
                } else {
                    exact.entry(p.source()).or_default().push(i);
                }
            }
        }
        Self { rules, exact, wild }
    }

    pub fn rule(&self, i: usize) -> &'r EffectRule {
        self.rules[i]
    }

    /// Indices of rules matching a call, in pack order. Client rules need the call to look
    /// like an HTTP call unless the pattern is a qualified path; when it does, `*.get` also
    /// matches a bare `get` left behind when the adapter drops a computed receiver.
    pub fn matching(&self, call: &CallSite, candidates: &[String], is_async: bool) -> Vec<usize> {
        let shaped = client_shaped(call);
        let mut hits: Vec<usize> = Vec::new();
        for c in candidates {
            if let Some(list) = self.exact.get(c.as_str()) {
                let qualified = c.contains("::") || c.contains('.');
                hits.extend(
                    list.iter()
                        .copied()
                        .filter(|&i| !self.rules[i].client || shaped || qualified),
                );
            }
        }
        for (p, i) in &self.wild {
            let rule = self.rules[*i];
            let hit = candidates
                .iter()
                .any(|c| p.matches(c) || (rule.client && shaped && p.matches(&format!("_.{c}"))));
            if hit && (!rule.client || shaped) {
                hits.push(*i);
            }
        }
        hits.sort_unstable();
        hits.dedup();
        hits.retain(|&i| !self.rules[i].async_only || is_async);
        hits
    }
}

pub(crate) fn build_rule_sets(ctx: &mut Ctx) {
    for file in ctx.files {
        let qualified: HashSet<&str> = file
            .symbols
            .iter()
            .filter_map(|s| s.body.as_ref())
            .flat_map(|b| b.calls.iter())
            .filter_map(|c| c.callee.split_once("::").map(|(root, _)| root))
            .collect();
        let rules = ctx
            .packs
            .effects()
            .filter(|r| {
                r.language.is_none_or(|l| {
                    l == file.language
                        || (l == Language::TypeScript && file.language == Language::JavaScript)
                })
            })
            .filter(|r| {
                r.requires_import.as_ref().is_none_or(|req| {
                    file.imports.iter().any(|i| i.path.contains(req.as_str()))
                        || qualified.contains(req.as_str())
                })
            })
            .collect();
        ctx.rule_sets.push(RuleSet::new(rules));
    }
}

/// Wildcard client patterns (`*.get`) only fire when the call looks like an HTTP call, so a
/// `map.get(key)` in a file that imports reqwest stays silent.
fn client_shaped(call: &CallSite) -> bool {
    if call.string_args.iter().any(|a| parse_url(a).is_some()) {
        return true;
    }
    let receiver = call.receiver.as_deref().unwrap_or("").to_ascii_lowercase();
    ["client", "http", "api", "session", "req", "fetch", "agent"]
        .iter()
        .any(|k| receiver.contains(k))
}

pub(crate) fn is_spawn_call(ctx: &Ctx, f: usize, call: &CallSite) -> bool {
    let fi = ctx.fns[f].file;
    let candidates = expanded_callees(ctx, fi, &call.callee);
    let set = &ctx.rule_sets[fi];
    set.matching(call, &candidates, ctx.symbol(f).is_async)
        .into_iter()
        .any(|i| set.rule(i).spawn)
}

struct RouteRef {
    method: String,
    path: String,
    node: usize,
}

fn route_node(ctx: &mut Ctx, method: &str, path: &str) -> usize {
    let path = normalise_path(path);
    let id = format!("route:{method} {path}");
    let name = format!("{method} {path}");
    ctx.synthetic(NodeKind::Route, id, name)
}

fn parse_attribute(attr: &str) -> (String, String) {
    match attr.split_once('(') {
        Some((name, rest)) => (
            name.trim().to_owned(),
            rest.trim_end_matches(')').to_owned(),
        ),
        None => (attr.trim().to_owned(), String::new()),
    }
}

fn quoted_strings(args: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = args.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '"' || c == '\'' || c == '`' {
            let mut s = String::new();
            for d in chars.by_ref() {
                if d == c {
                    break;
                }
                s.push(d);
            }
            out.push(s);
        }
    }
    out
}

fn methods_in_args(args: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for word in args.split(|c: char| !c.is_ascii_alphabetic()) {
        if METHODS.contains(&word) && !out.iter().any(|m| m == word) {
            out.push(word.to_owned());
        }
        let upper = word.to_ascii_uppercase();
        if word.len() > 2
            && METHODS.contains(&upper.as_str())
            && args.contains(&format!("\"{word}\""))
            && !out.contains(&upper)
        {
            out.push(upper);
        }
    }
    out
}

pub(crate) fn routes(ctx: &mut Ctx) {
    attribute_routes(ctx);
    builder_routes(ctx);
}

fn attribute_routes(ctx: &mut Ctx) {
    for f in 0..ctx.fns.len() {
        let sym = ctx.symbol(f);
        let language = ctx.files[ctx.fns[f].file].language;
        let mut found: Vec<(String, String)> = Vec::new();
        for attr in &sym.attributes {
            let (name, args) = parse_attribute(attr);
            let rule_hit = ctx
                .packs
                .routes()
                .filter(|r| r.style == RouteStyle::Attribute)
                .filter(|r| r.language.is_none_or(|l| l == language))
                .any(|r| r.patterns.iter().any(|p| p.matches(&name)));
            if !rule_hit {
                continue;
            }
            let strings = quoted_strings(&args);
            let path = strings
                .iter()
                .find(|s| s.starts_with('/') || s.is_empty())
                .cloned()
                .unwrap_or_else(|| "/".to_owned());
            if !path.starts_with('/') {
                continue;
            }
            let last = name.rsplit(['.', ':']).next().unwrap_or(&name);
            let mut methods = method_from_name(last)
                .map(|m| vec![m.to_owned()])
                .unwrap_or_default();
            if methods.is_empty() {
                methods = methods_in_args(&args);
            }
            if methods.is_empty() {
                methods.push(if last.eq_ignore_ascii_case("websocket") {
                    "GET".to_owned()
                } else {
                    "ANY".to_owned()
                });
            }
            for m in methods {
                found.push((m, path.clone()));
            }
        }
        for (method, path) in found {
            let route = route_node(ctx, &method, &path);
            let handler = ctx.fns[f].node;
            ctx.simple_edge(route, handler, EdgeKind::Handles, Confidence::Exact);
        }
    }
}

fn builder_routes(ctx: &mut Ctx) {
    for f in 0..ctx.fns.len() {
        let sym = ctx.symbol(f);
        let Some(body) = &sym.body else { continue };
        let fi = ctx.fns[f].file;
        let language = ctx.files[fi].language;
        let mut registrations: Vec<(Vec<String>, String, Vec<usize>, bool)> = Vec::new();
        for (ci, call) in body.calls.iter().enumerate() {
            let Some(path) = call.string_args.iter().find(|s| s.starts_with('/')) else {
                continue;
            };
            let candidates = expanded_callees(ctx, fi, &call.callee);
            let rule = ctx
                .packs
                .routes()
                .filter(|r| r.style == RouteStyle::Builder)
                .filter(|r| r.language.is_none_or(|l| l == language))
                .find(|r| {
                    r.patterns.iter().any(|p| {
                        candidates
                            .iter()
                            .any(|c| p.matches(c) || p.matches(&format!("_.{c}")))
                    })
                });
            // Arguments of a multi-line registration run until the next registration or
            // the first middleware/state/fallback attachment. `post(h).layer(limit)` keeps
            // its line: the layer decorates the handler written before it.
            let window_end = body
                .calls
                .iter()
                .filter(|c| c.line > call.line)
                .filter_map(|c| {
                    if literal_path(c).is_some() {
                        Some(c.line)
                    } else if attaches_middleware(c) {
                        let after_handler = decorates_handler(c)
                            && body.calls.iter().any(|m| {
                                m.line == c.line
                                    && m.receiver.is_none()
                                    && method_from_name(&m.name).is_some()
                            });
                        Some(if after_handler { c.line + 1 } else { c.line })
                    } else {
                        None
                    }
                })
                .min()
                .unwrap_or(u32::MAX)
                .min(call.line + REGISTRATION_WINDOW);
            let same_line: Vec<(usize, &CallSite)> = body
                .calls
                .iter()
                .enumerate()
                .filter(|(j, c)| *j != ci && c.line >= call.line && c.line < window_end)
                .collect();
            // `r.add("/x", post(handler))`: a method router built alongside marks a
            // registration even when the registering call is a project-specific wrapper.
            let method_router = method_from_name(&call.name).is_none()
                && same_line
                    .iter()
                    .any(|(_, c)| c.receiver.is_none() && method_from_name(&c.name).is_some());
            let Some(default_method) = rule
                .map(|r| r.method.clone())
                .or_else(|| method_router.then_some(None))
            else {
                continue;
            };
            let last = callee_segments(&call.callee)
                .last()
                .cloned()
                .unwrap_or_default();
            let mut methods: Vec<String> = method_from_name(&last)
                .map(|m| vec![m.to_owned()])
                .unwrap_or_default();
            if methods.is_empty() {
                for (_, c) in &same_line {
                    if let Some(m) = method_from_name(&c.name)
                        && !methods.iter().any(|x| x == m)
                    {
                        methods.push(m.to_owned());
                    }
                }
            }
            if methods.is_empty() {
                methods.push(default_method.unwrap_or_else(|| "ANY".to_owned()));
            }
            let mut handlers = Vec::new();
            for (_, c) in &same_line {
                if method_from_name(&c.name).is_some() || c.in_closure {
                    continue;
                }
                for (t, _) in resolve_call(ctx, f, c) {
                    // axum handlers are async; sync calls alongside (`body_limit()`) are config.
                    let plausible = language != Language::Rust || ctx.symbol(t).is_async;
                    if t != f && plausible && !handlers.contains(&t) {
                        handlers.push(t);
                    }
                }
            }
            let end = body
                .calls
                .iter()
                .filter(|c| !c.in_closure && c.line > call.line)
                .map(|c| c.line)
                .min()
                .unwrap_or(u32::MAX);
            let inline = handlers.is_empty()
                && body
                    .calls
                    .iter()
                    .any(|c| c.in_closure && c.line >= call.line && c.line < end);
            registrations.push((methods, path.clone(), handlers, inline));
        }
        for (methods, path, handlers, inline) in registrations {
            for method in methods {
                let route = route_node(ctx, &method, &path);
                if handlers.is_empty() {
                    if inline {
                        let node = ctx.fns[f].node;
                        ctx.simple_edge(route, node, EdgeKind::Handles, Confidence::Heuristic);
                    } else {
                        ctx.count("routes registered without a resolvable handler");
                    }
                }
                for &h in &handlers {
                    let node = ctx.fns[h].node;
                    ctx.simple_edge(route, node, EdgeKind::Handles, Confidence::Name);
                }
            }
        }
    }
}

/// Lines a route registration's arguments may span.
const REGISTRATION_WINDOW: u32 = 12;

/// Calls whose arguments wrap or configure a router rather than handle a route:
/// axum `.layer`/`.with_state`/`.fallback`, tower `make_span_with`, express `app.use`,
/// chi/gin `.Use`, FastAPI `add_middleware`.
const MIDDLEWARE_CALLS: &[&str] = &[
    "layer",
    "route_layer",
    "with_state",
    "fallback",
    "fallback_service",
    "method_not_allowed_fallback",
    "make_span_with",
    "on_request",
    "on_response",
    "on_failure",
    "from_fn",
    "from_fn_with_state",
    "map_request",
    "map_response",
    "wrap",
    "wrap_fn",
    "use",
    "Use",
    "With",
    "add_middleware",
    "middleware",
];

fn attaches_middleware(call: &CallSite) -> bool {
    MIDDLEWARE_CALLS.contains(&call.name.as_str())
}

/// Wraps whatever precedes it in the chain instead of taking a handler argument.
fn decorates_handler(call: &CallSite) -> bool {
    matches!(call.name.as_str(), "layer" | "route_layer" | "with_state")
}

fn known_routes(ctx: &Ctx) -> Vec<RouteRef> {
    ctx.nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| n.kind == NodeKind::Route)
        .filter_map(|(i, n)| {
            let (method, path) = n.name.split_once(' ')?;
            Some(RouteRef {
                method: method.to_owned(),
                path: path.to_owned(),
                node: i,
            })
        })
        .collect()
}

/// A path made only of placeholders (`/{}`) names no route in particular.
fn has_literal_segment(path: &str) -> bool {
    path.split('/').any(|s| !s.is_empty() && s != "{}")
}

fn match_routes(routes: &[RouteRef], method: Option<&str>, path: &str) -> Vec<usize> {
    if !has_literal_segment(path) {
        return Vec::new();
    }
    let method_ok = |r: &RouteRef| method.is_none_or(|m| r.method == "ANY" || r.method == m);
    let exact: Vec<usize> = routes
        .iter()
        .filter(|r| method_ok(r) && r.path == path)
        .map(|r| r.node)
        .collect();
    if !exact.is_empty() {
        return exact;
    }
    let suffix: Vec<usize> = routes
        .iter()
        .filter(|r| method_ok(r) && path != "/" && has_literal_segment(&r.path))
        .filter(|r| r.path.ends_with(path) || path.ends_with(&r.path))
        .map(|r| r.node)
        .collect();
    if suffix.len() <= 3 {
        suffix
    } else {
        Vec::new()
    }
}

/// Hits spanning several distinct paths: a suffix match that could mean any of them.
fn ambiguous_routes(routes: &[RouteRef], hits: &[usize]) -> bool {
    let paths: HashSet<&str> = routes
        .iter()
        .filter(|r| hits.contains(&r.node))
        .map(|r| r.path.as_str())
        .collect();
    paths.len() > 1
}

fn effect_edge(
    ctx: &mut Ctx,
    from: usize,
    to: usize,
    kind: EdgeKind,
    call: &CallSite,
    confidence: Confidence,
) {
    let mut edge = Edge::new(
        ctx.nodes[from].id.clone(),
        ctx.nodes[to].id.clone(),
        kind,
        confidence,
    );
    edge.line = Some(call.line);
    edge.loop_depth = call.loop_depth;
    edge.conditional = call.conditional;
    edge.awaited = call.awaited;
    ctx.add_edge(from, to, edge);
}

/// Enum variants of types defined in the repo that are both handed to a method call
/// somewhere and matched on elsewhere: the messages of the codebase. One-sided sightings
/// are plain enum use; third-party enums (`serde_json::Value`) are data, not messages.
fn message_topics(ctx: &Ctx) -> HashSet<String> {
    let mut sent: HashSet<&str> = HashSet::new();
    let mut matched: HashSet<&str> = HashSet::new();
    for f in 0..ctx.fns.len() {
        let Some(body) = &ctx.symbol(f).body else {
            continue;
        };
        for call in &body.calls {
            let Some(variant) = call.string_args.first() else {
                continue;
            };
            match call.callee.as_str() {
                MESSAGE_SITE => sent.insert(variant),
                MATCH_SITE => matched.insert(variant),
                _ => false,
            };
        }
    }
    sent.intersection(&matched)
        .filter(|v| {
            v.split_once("::")
                .is_some_and(|(ty, _)| ctx.type_by_name.contains_key(ty))
        })
        .map(|v| (*v).to_owned())
        .collect()
}

pub(crate) const MESSAGE_SITE: &str = "<message>";
pub(crate) const MATCH_SITE: &str = "<match>";

pub(crate) fn effects(ctx: &mut Ctx) {
    let routes = known_routes(ctx);
    let topics = message_topics(ctx);
    let mut wrappers: HashMap<usize, String> = HashMap::new();
    for f in 0..ctx.fns.len() {
        let sym = ctx.symbol(f);
        let Some(body) = &sym.body else { continue };
        let fi = ctx.fns[f].file;
        let from = ctx.fns[f].node;
        let mut direct: Vec<Effect> = Vec::new();
        let mut seen: HashSet<(EffectKind, u32, String)> = HashSet::new();
        let mut used: BTreeMap<String, usize> = BTreeMap::new();
        for call in &body.calls {
            if matches!(call.callee.as_str(), MESSAGE_SITE | MATCH_SITE)
                && !call.string_args.first().is_some_and(|v| topics.contains(v))
            {
                continue;
            }
            let candidates = expanded_callees(ctx, fi, &call.callee);
            if let Some(first) = callee_segments(&call.callee).first()
                && let Some(&imp) = ctx.scopes[fi].bindings.get(first)
                && let Some(dep) = ctx.scopes[fi].imports[imp].dependency.clone()
                && let Some(&dep_node) = ctx.node_index.get(&dep)
            {
                used.entry(dep).or_insert(dep_node);
            }
            let hits = ctx.rule_sets[fi].matching(call, &candidates, sym.is_async);
            for i in hits {
                let rule = ctx.rule_sets[fi].rule(i);
                let mut kind = rule.kind.effect_kind();
                let mut target: Option<String> = None;
                match rule.target {
                    Target::None => {}
                    Target::Arg0 => {
                        target = call.string_args.first().cloned();
                        if let Some(t) = &target
                            && matches!(kind, EffectKind::Publish | EffectKind::Subscribe)
                            && !t.is_empty()
                        {
                            let topic =
                                ctx.synthetic(NodeKind::Topic, format!("topic:{t}"), t.clone());
                            let edge_kind = if kind == EffectKind::Publish {
                                EdgeKind::Publishes
                            } else {
                                EdgeKind::Subscribes
                            };
                            effect_edge(ctx, from, topic, edge_kind, call, Confidence::Heuristic);
                        }
                    }
                    Target::Sql => {
                        if let Some(facts) = call.string_args.iter().find_map(|a| sql::analyse(a)) {
                            if facts.transaction {
                                kind = EffectKind::Lock;
                            } else if rule.kind == RuleKind::Db {
                                kind = facts.kind();
                            }
                            let mut names: Vec<String> = facts.changed().cloned().collect();
                            names.extend(facts.reads.iter().cloned());
                            names.dedup();
                            if !names.is_empty() {
                                target = Some(names.join(","));
                            }
                            if facts.locks
                                && seen.insert((EffectKind::Lock, call.line, rule.id.clone()))
                            {
                                direct.push(Effect {
                                    kind: EffectKind::Lock,
                                    target: target.clone(),
                                    via: call.callee.clone(),
                                    line: call.line,
                                    loop_depth: call.loop_depth,
                                    awaited: call.awaited,
                                    rule: rule.id.clone(),
                                });
                            }
                            if !facts.assignments.is_empty() || !facts.conditions.is_empty() {
                                ctx.sql_sites.push(SqlSite {
                                    function: f,
                                    line: call.line,
                                    assignments: facts.assignments.clone(),
                                    conditions: facts.conditions.clone(),
                                });
                            }
                            for t in &facts.reads {
                                let table =
                                    ctx.synthetic(NodeKind::Table, format!("table:{t}"), t.clone());
                                effect_edge(
                                    ctx,
                                    from,
                                    table,
                                    EdgeKind::Reads,
                                    call,
                                    Confidence::Heuristic,
                                );
                            }
                            for t in facts.changed() {
                                let table =
                                    ctx.synthetic(NodeKind::Table, format!("table:{t}"), t.clone());
                                effect_edge(
                                    ctx,
                                    from,
                                    table,
                                    EdgeKind::Writes,
                                    call,
                                    Confidence::Heuristic,
                                );
                            }
                        }
                    }
                    Target::Url => {
                        if let Some((raw, parts)) = call
                            .string_args
                            .iter()
                            .find_map(|a| parse_url(a).map(|p| (a, p)))
                        {
                            target = Some(match &parts.host {
                                Some(h) => format!("{h}{}", parts.path),
                                None => parts.path.clone(),
                            });
                            if rule.client {
                                let placeholder_base = raw.starts_with('{') || raw.starts_with('$');
                                if placeholder_base {
                                    let prefix = if parts.path == "/" {
                                        String::new()
                                    } else {
                                        parts.path.clone()
                                    };
                                    wrappers.entry(f).or_insert(prefix);
                                }
                                let last = callee_segments(&call.callee)
                                    .last()
                                    .cloned()
                                    .unwrap_or_default();
                                let method = method_from_prefix(&last);
                                let hits = if placeholder_base && parts.path == "/" {
                                    Vec::new()
                                } else {
                                    match_routes(&routes, method, &parts.path)
                                };
                                if !hits.is_empty() {
                                    let candidates = hits.len();
                                    let ambiguous = ambiguous_routes(&routes, &hits);
                                    for route in hits {
                                        let mut edge = Edge::new(
                                            ctx.nodes[from].id.clone(),
                                            ctx.nodes[route].id.clone(),
                                            EdgeKind::Requests,
                                            Confidence::Heuristic,
                                        );
                                        edge.line = Some(call.line);
                                        edge.loop_depth = call.loop_depth;
                                        edge.conditional = call.conditional;
                                        edge.awaited = call.awaited;
                                        edge.attrs.insert("url".into(), raw.clone());
                                        if ambiguous {
                                            edge.attrs.insert(
                                                "candidates".into(),
                                                candidates.to_string(),
                                            );
                                        }
                                        ctx.add_edge(from, route, edge);
                                    }
                                } else if let Some(host) = &parts.host {
                                    let host_node = ctx.synthetic(
                                        NodeKind::Host,
                                        format!("host:{host}"),
                                        host.clone(),
                                    );
                                    effect_edge(
                                        ctx,
                                        from,
                                        host_node,
                                        EdgeKind::Requests,
                                        call,
                                        Confidence::Heuristic,
                                    );
                                }
                            }
                        }
                    }
                }
                let key = (kind, call.line, rule.id.clone());
                if !seen.insert(key) {
                    continue;
                }
                direct.push(Effect {
                    kind,
                    target,
                    via: call.callee.clone(),
                    line: call.line,
                    loop_depth: call.loop_depth,
                    awaited: call.awaited,
                    rule: rule.id.clone(),
                });
            }
        }
        for (_, dep) in used {
            ctx.simple_edge(from, dep, EdgeKind::Uses, Confidence::Exact);
        }
        direct.sort_by_key(|e| (e.line, e.kind));
        ctx.nodes[from].effects.direct = direct;
    }
    wrapper_requests(ctx, &routes, wrappers);
}

/// `post` → POST, and `postForm` / `get_json` / `deleteOne` → the verb they start with.
fn method_from_prefix(name: &str) -> Option<&'static str> {
    if let Some(m) = method_from_name(name) {
        return Some(m);
    }
    let verb_len = name
        .find(|c: char| c.is_ascii_uppercase() || c == '_')
        .unwrap_or(name.len());
    let verb = &name[..verb_len];
    (verb_len < name.len())
        .then(|| method_from_name(verb))
        .flatten()
}

fn has_string_param(ctx: &Ctx, f: usize) -> bool {
    ctx.symbol(f).params.iter().any(|p| match &p.ty {
        None => true,
        Some(ty) => {
            let ty = ty.to_ascii_lowercase();
            ty.contains("str") || ty.contains("string")
        }
    })
}

fn literal_path(call: &CallSite) -> Option<&str> {
    call.string_args
        .iter()
        .map(String::as_str)
        .find(|a| a.starts_with('/'))
}

fn literal_method(call: &CallSite) -> Option<&'static str> {
    call.string_args.iter().find_map(|a| method_from_name(a))
}

/// A function that issues a client call on a URL whose base is a placeholder
/// (`fetch(`${API_URL}${path}`)`) and takes a string parameter is a request wrapper: each
/// caller passing a literal `/path` is itself a client of the matching route. Functions that
/// pass their own parameter through to a wrapper are wrappers too (one hop).
fn wrapper_requests(ctx: &mut Ctx, routes: &[RouteRef], mut wrappers: HashMap<usize, String>) {
    wrappers.retain(|&f, _| has_string_param(ctx, f));
    if wrappers.is_empty() {
        return;
    }
    let mut hop: Vec<(usize, String)> = Vec::new();
    for f in 0..ctx.fns.len() {
        if wrappers.contains_key(&f) || !has_string_param(ctx, f) {
            continue;
        }
        let Some(body) = &ctx.symbol(f).body else {
            continue;
        };
        let calls: Vec<CallSite> = body
            .calls
            .iter()
            .filter(|c| literal_path(c).is_none())
            .cloned()
            .collect();
        for call in &calls {
            if let Some(prefix) = resolve_call(ctx, f, call)
                .into_iter()
                .find_map(|(t, _)| wrappers.get(&t).cloned())
            {
                hop.push((f, prefix));
                break;
            }
        }
    }
    wrappers.extend(hop);

    for f in 0..ctx.fns.len() {
        let Some(body) = &ctx.symbol(f).body else {
            continue;
        };
        let calls: Vec<CallSite> = body
            .calls
            .iter()
            .filter(|c| literal_path(c).is_some())
            .cloned()
            .collect();
        let from = ctx.fns[f].node;
        for call in &calls {
            let Some(prefix) = resolve_call(ctx, f, call)
                .into_iter()
                .find_map(|(t, _)| wrappers.get(&t).cloned())
            else {
                continue;
            };
            let literal = literal_path(call).unwrap_or("/");
            let path = normalise_path(&format!("{prefix}{literal}"));
            let method = literal_method(call);
            let mut hits = match_routes(routes, method, &path);
            if method.is_none() && hits.len() > 1 {
                let gets: Vec<usize> = hits
                    .iter()
                    .copied()
                    .filter(|&r| routes.iter().any(|x| x.node == r && x.method == "GET"))
                    .collect();
                if !gets.is_empty() {
                    hits = gets;
                }
            }
            let candidates = hits.len();
            let ambiguous = ambiguous_routes(routes, &hits);
            for route in hits {
                let mut edge = Edge::new(
                    ctx.nodes[from].id.clone(),
                    ctx.nodes[route].id.clone(),
                    EdgeKind::Requests,
                    Confidence::Heuristic,
                );
                edge.line = Some(call.line);
                edge.loop_depth = call.loop_depth;
                edge.conditional = call.conditional;
                edge.awaited = call.awaited;
                edge.attrs.insert("url".into(), literal.to_owned());
                edge.attrs.insert("via".into(), call.callee.clone());
                if ambiguous {
                    edge.attrs
                        .insert("candidates".into(), candidates.to_string());
                }
                ctx.add_edge(from, route, edge);
            }
            let node = &mut ctx.nodes[from];
            if !node
                .effects
                .direct
                .iter()
                .any(|e| e.line == call.line && e.kind == EffectKind::Network)
            {
                node.effects.direct.push(Effect {
                    kind: EffectKind::Network,
                    target: Some(path.clone()),
                    via: call.callee.clone(),
                    line: call.line,
                    loop_depth: call.loop_depth,
                    awaited: call.awaited,
                    rule: "common.http.client_wrapper".into(),
                });
                node.effects.direct.sort_by_key(|e| (e.line, e.kind));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn routes() -> Vec<RouteRef> {
        [
            ("GET", "/{}"),
            ("GET", "/api/v1/album/{}"),
            ("GET", "/api/v1/album"),
        ]
        .iter()
        .enumerate()
        .map(|(i, (m, p))| RouteRef {
            method: (*m).to_owned(),
            path: (*p).to_owned(),
            node: i,
        })
        .collect()
    }

    #[test]
    fn placeholder_only_paths_match_no_route() {
        let routes = routes();
        assert!(match_routes(&routes, Some("GET"), "/{}").is_empty());
        assert!(match_routes(&routes, Some("GET"), "/{}/{}").is_empty());
        assert_eq!(
            match_routes(&routes, Some("GET"), "/api/v1/album/{}"),
            vec![1]
        );
        assert_eq!(match_routes(&routes, Some("GET"), "/album"), vec![2]);
    }
}
