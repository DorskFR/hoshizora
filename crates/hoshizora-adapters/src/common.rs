//! Language-neutral tree walking shared by every adapter.

use hoshizora_model::{Body, CallSite, Span, Symbol, SymbolKind, Visibility};
use regex::Regex;
use std::sync::LazyLock;
use tree_sitter::{Language, Node, Parser, Tree};

pub fn parse_tree(language: &Language, source: &str) -> anyhow::Result<Tree> {
    let mut parser = Parser::new();
    parser.set_language(language)?;
    parser
        .parse(source, None)
        .ok_or_else(|| anyhow::anyhow!("tree-sitter returned no tree"))
}

pub fn text<'a>(node: Node, source: &'a str) -> &'a str {
    node.utf8_text(source.as_bytes()).unwrap_or("")
}

pub fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn strip_ws(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect()
}

pub fn line(node: Node) -> u32 {
    node.start_position().row as u32 + 1
}

pub fn span(node: Node) -> Span {
    Span {
        start_line: line(node),
        end_line: node.end_position().row as u32 + 1,
    }
}

pub fn line_count(source: &str) -> u32 {
    let n = source.lines().count() as u32;
    n.max(u32::from(!source.is_empty()))
}

pub fn named_children<'t>(node: Node<'t>) -> Vec<Node<'t>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).collect()
}

pub fn all_children<'t>(node: Node<'t>) -> Vec<Node<'t>> {
    let mut cursor = node.walk();
    node.children(&mut cursor).collect()
}

pub fn field_text(node: Node, field: &str, source: &str) -> Option<String> {
    node.child_by_field_name(field)
        .map(|n| collapse_ws(text(n, source)))
}

pub fn has_child_kind(node: Node, kind: &str) -> bool {
    all_children(node).iter().any(|c| c.kind() == kind)
}

pub fn child_of_kind<'t>(node: Node<'t>, kind: &str) -> Option<Node<'t>> {
    all_children(node).into_iter().find(|c| c.kind() == kind)
}

/// Header text of a definition: everything before its `body` field, whitespace collapsed.
pub fn header(node: Node, source: &str, body_field: &str) -> String {
    let end = node
        .child_by_field_name(body_field)
        .map_or(node.end_byte(), |b| b.start_byte());
    let mut sig = collapse_ws(&source[node.start_byte()..end]);
    while sig.ends_with([':', ';', ' ', '{']) {
        sig.pop();
    }
    sig
}

/// Trailing segment of a dotted / `::` path, generics stripped.
pub fn last_segment(callee: &str) -> String {
    let base = callee.split('<').next().unwrap_or(callee);
    base.rsplit(['.', ':']).next().unwrap_or(base).to_string()
}

pub fn strip_generics(ty: &str) -> String {
    let mut depth = 0u32;
    let mut out = String::new();
    for c in ty.chars() {
        match c {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out.trim().to_string()
}

/// Joins preceding comment lines into one doc string, removing `marker` from each line.
pub fn clean_comment_lines(lines: &[&str], markers: &[&str]) -> Option<String> {
    let mut out = Vec::new();
    for raw in lines {
        for l in raw.lines() {
            let mut t = l.trim();
            if let Some(rest) = t.strip_suffix("*/") {
                t = rest.trim();
            }
            for m in markers {
                if let Some(rest) = t.strip_prefix(m) {
                    t = rest.trim();
                    break;
                }
            }
            out.push(t.to_string());
        }
    }
    let doc = out.join("\n").trim().to_string();
    (!doc.is_empty()).then_some(doc)
}

pub struct CallInfo {
    pub callee: String,
    pub receiver: Option<String>,
    pub string_args: Vec<String>,
}

impl CallInfo {
    /// Method call on `receiver`; a computed receiver (`a.b().c`, `xs[0].f`) is dropped so the
    /// callee is just the method name.
    pub fn method(callee: &str, receiver: Option<String>, string_args: Vec<String>) -> Self {
        let callee = strip_ws(callee);
        match receiver.map(|r| strip_ws(&r)) {
            Some(r) if r.contains(['(', '[', '{', '?', '!']) => Self {
                callee: last_segment(&callee),
                receiver: None,
                string_args,
            },
            receiver => Self {
                callee,
                receiver,
                string_args,
            },
        }
    }
}

/// Node-kind vocabulary a language hands to [`analyze_body`].
pub struct BodySpec<'a> {
    pub loops: &'a [&'a str],
    /// Nodes whose children (except the `condition` field) are conditional.
    pub conditionals: &'a [&'a str],
    pub branches: &'a [&'a str],
    /// Nodes whose arms are counted individually; one arm per node is subtracted.
    pub switches: &'a [&'a str],
    pub arms: &'a [&'a str],
    pub binary: &'a [&'a str],
    pub closures: &'a [&'a str],
    pub early_exits: &'a [&'a str],
    pub awaits: &'a [&'a str],
    /// Kinds that keep an await attached to a call it wraps (`a.b().c().await`).
    pub chain: &'a [&'a str],
    /// Nested definitions that stop descent (emitted as their own symbols).
    pub stop: &'a [&'a str],
    /// Argument kinds that name a function when passed bare (`get(list)`, `HandleFunc("/x", h)`);
    /// each is emitted as a call of its own on the same line.
    pub ref_kinds: &'a [&'a str],
    /// Literal containers (arrays, object entries) whose bare-identifier elements are also
    /// treated as function references.
    pub ref_containers: &'a [&'a str],
    /// Statements that spawn their expression (`go f()`): emitted as a call named after their
    /// keyword, with everything inside marked `in_closure`.
    pub spawn: &'a [&'a str],
    pub call: fn(Node, &str) -> Option<CallInfo>,
    /// Calls hidden inside a node tree-sitter does not parse (Rust macro token trees).
    pub expand: Option<fn(Node, &str) -> Vec<CallSite>>,
}

const NOT_A_FUNCTION: &[&str] = &[
    "self",
    "this",
    "cls",
    "None",
    "True",
    "False",
    "nil",
    "null",
    "undefined",
    "true",
    "false",
];

fn names_function(text: &str) -> bool {
    let last = last_segment(text);
    !last.is_empty()
        && !NOT_A_FUNCTION.contains(&text)
        && !last
            .chars()
            .all(|c| c.is_ascii_uppercase() || c == '_' || c.is_ascii_digit())
        && !last.starts_with(|c: char| c.is_ascii_digit())
}

#[derive(Clone, Copy, Default)]
struct State {
    loop_depth: u32,
    conditional: bool,
    in_closure: bool,
}

pub fn analyze_body(body: Node, source: &str, spec: &BodySpec) -> Body {
    analyze_nodes(&named_children(body), source, spec)
}

/// Body facts for a sequence of statements/expressions that do not share a block node.
pub fn analyze_nodes(nodes: &[Node], source: &str, spec: &BodySpec) -> Body {
    analyze_nodes_with(nodes, source, spec, &|_| true)
}

/// Like [`analyze_nodes`], emitting bare argument references only when `is_ref` accepts
/// the name, so data arguments (`send(body, files)`) are not mistaken for callbacks.
pub fn analyze_nodes_with(
    nodes: &[Node],
    source: &str,
    spec: &BodySpec,
    is_ref: &dyn Fn(&str) -> bool,
) -> Body {
    let mut out = Body {
        cyclomatic: 1,
        ..Body::default()
    };
    let last_stmt = nodes.last().map(|n| n.id());
    for child in nodes {
        walk(
            *child,
            source,
            spec,
            State::default(),
            &mut out,
            last_stmt,
            is_ref,
        );
    }
    out.cyclomatic = 1 + out.branches + out.loops;
    out.calls.sort_by_key(|c| c.line);
    out
}

/// Synthetic `<module>` symbol for top-level code, when it makes at least one call.
pub fn module_symbol(body: Body, lines: u32) -> Option<Symbol> {
    (!body.calls.is_empty()).then(|| Symbol {
        kind: SymbolKind::Function,
        name: "<module>".into(),
        local_name: "<module>".into(),
        parent: None,
        signature: "<module>".into(),
        params: vec![],
        returns: None,
        doc: None,
        span: Span {
            start_line: 1,
            end_line: lines.max(1),
        },
        is_async: false,
        visibility: Visibility::Private,
        attributes: vec![],
        implements: vec![],
        body: Some(body),
    })
}

/// Appends `extra` into `symbols`' `<module>` entry, or pushes it as a new one.
pub fn merge_module_symbol(symbols: &mut Vec<Symbol>, extra: Symbol) {
    match symbols.iter_mut().find(|s| s.name == "<module>") {
        Some(existing) => {
            let (Some(a), Some(b)) = (existing.body.as_mut(), extra.body) else {
                return;
            };
            a.calls.extend(b.calls);
            a.calls.sort_by_key(|c| c.line);
            a.branches += b.branches;
            a.loops += b.loops;
            a.max_loop_depth = a.max_loop_depth.max(b.max_loop_depth);
            a.early_exits += b.early_exits;
            a.closures += b.closures;
            a.cyclomatic = 1 + a.branches + a.loops;
        }
        None => symbols.push(extra),
    }
}

fn walk(
    node: Node,
    source: &str,
    spec: &BodySpec,
    mut st: State,
    out: &mut Body,
    last_stmt: Option<usize>,
    is_ref: &dyn Fn(&str) -> bool,
) {
    let kind = node.kind();
    if spec.stop.contains(&kind) {
        return;
    }
    if spec.loops.contains(&kind) {
        out.loops += 1;
        st.loop_depth += 1;
        out.max_loop_depth = out.max_loop_depth.max(st.loop_depth);
    }
    if spec.closures.contains(&kind) {
        out.closures += 1;
        st.in_closure = true;
    }
    if spec.branches.contains(&kind) || spec.arms.contains(&kind) {
        out.branches += 1;
    }
    if spec.switches.contains(&kind) {
        out.branches = out.branches.saturating_sub(1);
    }
    if spec.binary.contains(&kind) {
        let op = node
            .child_by_field_name("operator")
            .map(|o| text(o, source))
            .unwrap_or("&&");
        if matches!(op, "&&" | "||" | "and" | "or" | "??") {
            out.branches += 1;
        }
    }
    if spec.early_exits.contains(&kind) && Some(node.id()) != last_stmt {
        out.early_exits += 1;
    }
    if spec.spawn.contains(&kind) {
        let keyword = node.child(0).map_or(kind, |k| text(k, source));
        out.calls
            .push(site(keyword.to_string(), None, vec![], node, false, st));
        st.in_closure = true;
    }
    if let Some(info) = (spec.call)(node, source) {
        let callee = info.callee.replace("?.", ".");
        let awaited = is_awaited(node, spec);
        let at = node
            .child_by_field_name("arguments")
            .or_else(|| child_of_kind(node, "token_tree"))
            .unwrap_or(node);
        out.calls.push(site(
            callee,
            info.receiver,
            info.string_args,
            at,
            awaited,
            st,
        ));
        if let Some(expand) = spec.expand {
            for mut c in expand(node, source) {
                c.loop_depth += st.loop_depth;
                c.conditional |= st.conditional;
                c.in_closure |= st.in_closure;
                c.awaited |= awaited;
                out.calls.push(c);
            }
        }
        if let Some(args) = node.child_by_field_name("arguments") {
            emit_refs(&named_children(args), source, spec, st, out, is_ref);
        }
    }
    if spec.ref_containers.contains(&kind) {
        let elements = match node.child_by_field_name("value") {
            Some(v) => vec![v],
            None => named_children(node),
        };
        emit_refs(&elements, source, spec, st, out, is_ref);
    }
    let is_cond = spec.conditionals.contains(&kind);
    let guard = node.child_by_field_name("condition").map(|n| n.id());
    for child in named_children(node) {
        let mut cst = st;
        if is_cond && guard != Some(child.id()) {
            cst.conditional = true;
        }
        walk(child, source, spec, cst, out, last_stmt, is_ref);
    }
}

fn emit_refs(
    nodes: &[Node],
    source: &str,
    spec: &BodySpec,
    st: State,
    out: &mut Body,
    is_ref: &dyn Fn(&str) -> bool,
) {
    for n in nodes {
        let t = strip_ws(text(*n, source));
        if spec.ref_kinds.contains(&n.kind()) && names_function(&t) && is_ref(&t) {
            out.calls.push(site(t, None, vec![], *n, false, st));
        }
    }
}

fn site(
    callee: String,
    receiver: Option<String>,
    string_args: Vec<String>,
    node: Node,
    awaited: bool,
    st: State,
) -> CallSite {
    CallSite {
        name: last_segment(&callee),
        callee,
        receiver,
        string_args,
        line: line(node),
        loop_depth: st.loop_depth,
        awaited,
        conditional: st.conditional,
        in_closure: st.in_closure,
    }
}

/// Adds `ui:<event>` to every function symbol named in `handlers` (`(event, identifiers)`).
pub fn apply_ui_handlers(symbols: &mut [Symbol], handlers: &[(String, Vec<String>)]) {
    for (event, idents) in handlers {
        let attr = format!("ui:{event}");
        for sym in symbols.iter_mut() {
            if matches!(sym.kind, SymbolKind::Function | SymbolKind::Method)
                && idents.contains(&sym.name)
                && !sym.attributes.contains(&attr)
            {
                sym.attributes.push(attr.clone());
            }
        }
    }
}

/// Identifiers in a handler expression (`launch`, `() => launch(x)`, `obj.method`).
pub fn handler_idents(expr: &str) -> Vec<String> {
    expr.split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$'))
        .filter(|w| !w.is_empty() && !w.starts_with(|c: char| c.is_ascii_digit()))
        .map(str::to_string)
        .collect()
}

/// `onClick` / `onclick` / `on:click` / `@click` / `v-on:click` → `click`.
pub fn event_name(attr: &str) -> Option<String> {
    let rest = attr
        .strip_prefix("v-on:")
        .or_else(|| attr.strip_prefix('@'))
        .or_else(|| attr.strip_prefix("on:"))
        .or_else(|| attr.strip_prefix("on"))?;
    let rest = rest.split(['|', '.']).next().unwrap_or(rest);
    (!rest.is_empty()).then(|| rest.to_lowercase())
}

fn is_awaited(node: Node, spec: &BodySpec) -> bool {
    let mut cur = node;
    while let Some(p) = cur.parent() {
        if spec.awaits.contains(&p.kind()) {
            return true;
        }
        if !spec.chain.contains(&p.kind()) {
            return false;
        }
        cur = p;
    }
    false
}

/// Value of a string literal: quotes dropped, interpolations replaced by `{}`.
pub fn string_value(node: Node, source: &str, interpolation: &[&str], skip: &[&str]) -> String {
    let children = named_children(node);
    if children.is_empty() {
        let t = text(node, source);
        return t
            .trim_start_matches(['r', 'b', 'f', 'u', '#'])
            .trim_matches(['"', '\'', '`', '#'])
            .to_string();
    }
    let mut out = String::new();
    for c in children {
        let k = c.kind();
        if interpolation.contains(&k) {
            out.push_str("{}");
        } else if !skip.contains(&k) {
            out.push_str(text(c, source));
        }
    }
    out
}

/// String values among the direct children of an argument list, as judged by `string_arg`.
pub fn string_args(
    args: Option<Node>,
    source: &str,
    string_arg: fn(Node, &str) -> Option<String>,
) -> Vec<String> {
    let Some(args) = args else { return vec![] };
    if let Some(s) = string_arg(args, source) {
        return vec![s];
    }
    named_children(args)
        .into_iter()
        .filter_map(|a| string_arg(a, source))
        .collect()
}

static PLACEHOLDER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\{[^{}]*\}|%[-+ 0#]*\d*(?:\.\d+)?[sdifrxXeEgGcvqTtbopU]")
        .expect("placeholder regex")
});

/// `{name}`, `{:?}`, `%s`, `%d`, `%v`... → `{}`.
pub fn normalise_placeholders(s: &str) -> String {
    PLACEHOLDER.replace_all(s, "{}").into_owned()
}

/// `a + "/x" + b` → `{}/x{}`, provided at least one leaf is a literal.
pub fn concat_value(
    node: Node,
    source: &str,
    operator: &str,
    string_arg: fn(Node, &str) -> Option<String>,
) -> Option<String> {
    let root_op = node
        .child_by_field_name("operator")
        .map(|o| text(o, source));
    if root_op != Some(operator) {
        return None;
    }
    let mut parts = vec![];
    let mut pending = vec![node];
    while let Some(n) = pending.pop() {
        let op = n.child_by_field_name("operator").map(|o| text(o, source));
        if matches!(n.kind(), "binary_expression" | "binary_operator") && op == Some(operator) {
            for f in ["right", "left"] {
                if let Some(c) = n.child_by_field_name(f) {
                    pending.push(c);
                }
            }
        } else {
            parts.push(n);
        }
    }
    let mut seen_literal = false;
    let value: String = parts
        .into_iter()
        .map(|p| match string_arg(p, source) {
            Some(v) => {
                seen_literal = true;
                v
            }
            None => "{}".to_string(),
        })
        .collect();
    seen_literal.then_some(value)
}

/// Comments directly above `node` (optionally separated by `through` nodes such as attributes).
pub fn preceding_comments<'t>(
    node: Node<'t>,
    comment_kinds: &[&str],
    through: &[&str],
) -> Vec<Node<'t>> {
    let mut found = Vec::new();
    let mut cur = node;
    let mut expected_row = node.start_position().row;
    while let Some(prev) = cur.prev_sibling() {
        let k = prev.kind();
        if through.contains(&k) {
            expected_row = prev.start_position().row;
            cur = prev;
            continue;
        }
        if !comment_kinds.contains(&k) || prev.end_position().row + 1 < expected_row {
            break;
        }
        expected_row = prev.start_position().row;
        found.push(prev);
        cur = prev;
    }
    found.reverse();
    found
}

/// Preceding siblings of `kind` up to the first sibling that is neither `kind` nor a comment.
pub fn preceding_of_kind<'t>(node: Node<'t>, kind: &str, comment_kinds: &[&str]) -> Vec<Node<'t>> {
    let mut found = Vec::new();
    let mut cur = node;
    while let Some(prev) = cur.prev_sibling() {
        if prev.kind() == kind {
            found.push(prev);
        } else if !comment_kinds.contains(&prev.kind()) {
            break;
        }
        cur = prev;
    }
    found.reverse();
    found
}
