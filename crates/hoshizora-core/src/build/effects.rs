//! Effect rules, route rules and the client → route / SQL → table / topic linkers.

use std::collections::{BTreeMap, HashMap, HashSet};

use hoshizora_model::{
    CallSite, Confidence, Edge, EdgeKind, Effect, EffectKind, Language, NodeKind,
};

use super::Ctx;
use super::resolve::{callee_segments, resolve_call};
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
                r.requires_import
                    .as_ref()
                    .is_none_or(|req| file.imports.iter().any(|i| i.path.contains(req.as_str())))
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
            let Some(rule) = ctx
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
                })
            else {
                continue;
            };
            let same_line: Vec<(usize, &CallSite)> = body
                .calls
                .iter()
                .enumerate()
                .filter(|(j, c)| *j != ci && c.line == call.line)
                .collect();
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
                methods.push(rule.method.clone().unwrap_or_else(|| "ANY".to_owned()));
            }
            let mut handlers = Vec::new();
            for (_, c) in &same_line {
                if method_from_name(&c.name).is_some() || c.in_closure {
                    continue;
                }
                for (t, _) in resolve_call(ctx, f, c) {
                    if t != f && !handlers.contains(&t) {
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

fn match_routes(routes: &[RouteRef], method: Option<&str>, path: &str) -> Vec<usize> {
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
        .filter(|r| method_ok(r) && path != "/" && r.path != "/")
        .filter(|r| r.path.ends_with(path) || path.ends_with(&r.path))
        .map(|r| r.node)
        .collect();
    if suffix.len() <= 3 {
        suffix
    } else {
        Vec::new()
    }
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

pub(crate) fn effects(ctx: &mut Ctx) {
    let routes = known_routes(ctx);
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
                            if rule.kind == RuleKind::Db {
                                kind = if facts.is_write() {
                                    EffectKind::DbWrite
                                } else {
                                    EffectKind::DbRead
                                };
                            }
                            let mut names = facts.writes.clone();
                            names.extend(facts.reads.iter().cloned());
                            names.dedup();
                            target = Some(names.join(","));
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
                            for t in &facts.writes {
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
                                let method = method_from_name(&last);
                                let hits = if placeholder_base && parts.path == "/" {
                                    Vec::new()
                                } else {
                                    match_routes(&routes, method, &parts.path)
                                };
                                if !hits.is_empty() {
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
