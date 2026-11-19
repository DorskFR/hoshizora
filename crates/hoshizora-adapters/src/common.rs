//! Language-neutral tree walking shared by every adapter.

use hoshizora_model::{
    Body, CallSite, DefaultKind, Discrimination, Span, Symbol, SymbolKind, Visibility,
};
use regex::Regex;
use std::collections::HashMap;
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

/// Receiver recorded for a method called on an expression (`a.b()?.c()`, `f(x).c()`).
pub const EXPR_RECEIVER: &str = "<expr>";

impl CallInfo {
    /// Method call on `receiver`; a computed receiver (`a.b().c`, `xs[0].f`) is dropped so the
    /// callee is just the method name.
    pub fn method(callee: &str, receiver: Option<String>, string_args: Vec<String>) -> Self {
        let callee = strip_ws(callee);
        match receiver.map(|r| strip_ws(&r).replace("?.", ".")) {
            Some(r) if r.contains(['(', '[', '{', '?', '!']) => Self {
                callee: last_segment(&callee),
                receiver: Some(EXPR_RECEIVER.to_owned()),
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
    /// Leaf kinds that name a variable (`identifier`, `self`, shorthand struct fields).
    pub idents: &'a [&'a str],
    /// Statements that bind names, with the field holding the pattern or target.
    pub binders: &'a [(&'a str, &'a str)],
    /// Child kinds marking a mutable borrow of the identifiers beside them (`&mut x`).
    pub mutable: &'a [&'a str],
}

/// Fields whose identifier child names a member, callee or keyword rather than a variable.
const MEMBER_FIELDS: &[&str] = &[
    "field",
    "property",
    "attribute",
    "name",
    "function",
    "macro",
];

/// Subtrees that spell a path or a type, never a variable.
const PATH_KINDS: &[&str] = &[
    "scoped_identifier",
    "scoped_type_identifier",
    "generic_type",
    "type_arguments",
    "qualified_type",
];

/// Outermost expression the call is part of: the chain of member accesses, calls, awaits
/// and `?` it sits in.
fn chain_top<'t>(node: Node<'t>, spec: &BodySpec) -> Node<'t> {
    let mut cur = node;
    while let Some(p) = cur.parent() {
        if !spec.chain.contains(&p.kind()) && !spec.awaits.contains(&p.kind()) {
            break;
        }
        cur = p;
    }
    cur
}

fn idents(node: Node, source: &str, spec: &BodySpec, out: &mut Vec<String>) {
    if PATH_KINDS.contains(&node.kind()) {
        return;
    }
    if spec.idents.contains(&node.kind()) {
        let t = text(node, source);
        if !out.iter().any(|o| o == t) {
            out.push(t.to_owned());
        }
        return;
    }
    let mut cursor = node.walk();
    if !cursor.goto_first_child() {
        return;
    }
    loop {
        let child = cursor.node();
        let member = cursor
            .field_name()
            .is_some_and(|f| MEMBER_FIELDS.contains(&f))
            && spec.idents.contains(&child.kind());
        if !member {
            idents(child, source, spec, out);
        }
        if !cursor.goto_next_sibling() {
            break;
        }
    }
}

fn mutated(node: Node, source: &str, spec: &BodySpec, out: &mut Vec<String>) {
    if spec.mutable.is_empty() {
        return;
    }
    let mut cursor = node.walk();
    let children: Vec<Node> = node.children(&mut cursor).collect();
    if children.iter().any(|c| spec.mutable.contains(&c.kind())) {
        idents(node, source, spec, out);
        return;
    }
    for child in children {
        mutated(child, source, spec, out);
    }
}

/// Names bound by the statement enclosing `top`, if that statement is a binder.
fn binds(top: Node, source: &str, spec: &BodySpec) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = top;
    while let Some(p) = cur.parent() {
        if spec.stop.contains(&p.kind()) {
            break;
        }
        if let Some((_, field)) = spec.binders.iter().find(|(k, _)| *k == p.kind()) {
            if let Some(target) = p.child_by_field_name(field)
                && target.id() != cur.id()
            {
                idents(target, source, spec, &mut out);
            }
            break;
        }
        cur = p;
    }
    out
}

struct DataFlow {
    binds: Vec<String>,
    uses: Vec<String>,
    mutates: Vec<String>,
}

fn data_flow(top: Node, source: &str, spec: &BodySpec) -> DataFlow {
    let mut uses = Vec::new();
    idents(top, source, spec, &mut uses);
    let mut mutates = Vec::new();
    mutated(top, source, spec, &mut mutates);
    DataFlow {
        binds: binds(top, source, spec),
        uses,
        mutates,
    }
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
    let mut scope = Scope {
        last_stmt: nodes.last().map(|n| n.id()),
        is_ref,
        flows: HashMap::new(),
        headers: HashMap::new(),
        bound: Vec::new(),
        iterated: Vec::new(),
        guards: Vec::new(),
        small: nodes
            .first()
            .map(|n| literal_defaults(*n, source, spec))
            .unwrap_or_default(),
        near: HashMap::new(),
        parts: Vec::new(),
        grown: {
            let mut g = Vec::new();
            for n in nodes {
                grown(*n, source, &mut g);
            }
            g
        },
    };
    for child in nodes {
        walk(*child, source, spec, State::default(), &mut out, &mut scope);
    }
    out.cyclomatic = 1 + out.branches + out.loops;
    out.calls.sort_by_key(|c| c.line);
    for child in nodes {
        collect_refs(*child, None, "", source, spec, &mut out.refs);
    }
    out
}

/// Fields whose content is a callee, a declared name, a key or a pattern: never a reference.
const NON_REF_FIELDS: &[&str] = &[
    "field",
    "property",
    "attribute",
    "name",
    "function",
    "constructor",
    "macro",
    "key",
    "label",
    "type",
    "pattern",
    "parameters",
    "parameter",
    "alias",
];

/// Identifiers and member paths read as values anywhere under `node`: everything except
/// callees, receivers of a member access, declared names and assignment targets.
fn collect_refs(
    node: Node,
    field: Option<&str>,
    parent: &str,
    source: &str,
    spec: &BodySpec,
    out: &mut Vec<String>,
) {
    let kind = node.kind();
    if spec.stop.contains(&kind) {
        return;
    }
    let leaf = spec.ref_kinds.contains(&kind)
        || (kind.starts_with("shorthand") && spec.idents.contains(&kind));
    let member = leaf && kind != "identifier" && !kind.starts_with("shorthand");
    let path = leaf.then(|| strip_ws(text(node, source)).replace("?.", "."));
    let clean = path
        .as_deref()
        .is_some_and(|t| !t.contains(['(', '[', '{', '<', '?', '!', '"', '\'', '`']));
    let excluded = field.is_some_and(|f| NON_REF_FIELDS.contains(&f))
        || (!member
            && spec
                .binders
                .iter()
                .any(|(k, f)| *k == parent && Some(*f) == field))
        || (spec.ref_kinds.contains(&parent) && parent != "identifier");
    if clean && !excluded {
        let t = path.as_deref().unwrap_or_default();
        let prefixes = t.match_indices('.').skip(1).map(|(i, _)| &t[..i]);
        for p in prefixes.chain([t]) {
            if !NOT_A_FUNCTION.contains(&p) && !out.iter().any(|o| o == p) {
                out.push(p.to_owned());
            }
        }
    }
    if member && clean {
        return;
    }
    let mut cursor = node.walk();
    if !cursor.goto_first_child() {
        return;
    }
    loop {
        let child = cursor.node();
        if child.is_named() {
            collect_refs(child, cursor.field_name(), kind, source, spec, out);
        }
        if !cursor.goto_next_sibling() {
            break;
        }
    }
}

/// Synthetic `<module>` symbol for top-level code, when it calls or references anything.
pub fn module_symbol(body: Body, lines: u32) -> Option<Symbol> {
    (!body.calls.is_empty() || !body.refs.is_empty()).then(|| Symbol {
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
            for r in b.refs {
                if !a.refs.contains(&r) {
                    a.refs.push(r);
                }
            }
            a.branches += b.branches;
            a.loops += b.loops;
            a.max_loop_depth = a.max_loop_depth.max(b.max_loop_depth);
            a.pairwise |= b.pairwise;
            a.early_exits += b.early_exits;
            a.closures += b.closures;
            a.cyclomatic = 1 + a.branches + a.loops;
        }
        None => symbols.push(extra),
    }
}

/// Per-body walk state: the final statement (whose exit is not early), the reference
/// filter, and data flow cached per expression chain.
struct Scope<'a> {
    last_stmt: Option<usize>,
    is_ref: &'a dyn Fn(&str) -> bool,
    flows: HashMap<usize, DataFlow>,
    /// Loop headers (iterated expressions), walked with the state outside their loop.
    headers: HashMap<usize, State>,
    /// Names bound by the enclosing loops' patterns.
    bound: Vec<String>,
    /// Collections the enclosing loops that count towards the depth iterate over, as written.
    iterated: Vec<String>,
    /// Identifiers read by the conditions of the enclosing branches.
    guards: Vec<String>,
    /// Locals holding a few items: a short literal list, a stencil built from a window, a
    /// parameter defaulting to a literal tuple.
    small: Vec<String>,
    /// Locals a fixed reach past a point (`limit = min(n, at + GAP)`), with that point.
    near: HashMap<String, String>,
    /// Locals holding a part of a value (`let (a, b) = xs.split_at(mid)`, `left = xs[:mid]`).
    parts: Vec<String>,
    /// Locals the body grows (`xs.append(..)`, `xs += ..`): never small.
    grown: Vec<String>,
}

fn walk(
    node: Node,
    source: &str,
    spec: &BodySpec,
    mut st: State,
    out: &mut Body,
    scope: &mut Scope,
) {
    let kind = node.kind();
    if spec.stop.contains(&kind) {
        return;
    }
    if let Some(outer) = scope.headers.remove(&node.id()) {
        st = outer;
    }
    let bound_before = scope.bound.len();
    let iterated_before = scope.iterated.len();
    if spec.loops.contains(&kind) {
        out.loops += 1;
        let (pattern, iterable) = loop_parts(node);
        if let Some(it) = iterable {
            scope.headers.insert(it.id(), st);
        }
        if !bounded_loop(node, pattern, iterable, source, spec, scope) {
            st.loop_depth += 1;
            out.max_loop_depth = out.max_loop_depth.max(st.loop_depth);
            if let Some(base) = iterable
                .map(|it| unwrap_iterable(it, source))
                .filter(|b| !is_range(*b, source))
            {
                let over = strip_ws(text(base, source));
                out.pairwise |= scope.iterated.contains(&over);
                scope.iterated.push(over);
            }
        }
        if let Some(p) = pattern {
            pattern_names(p, source, spec.idents, &mut scope.bound);
        }
    }
    if !scope.bound.is_empty()
        && matches!(
            kind,
            "let_declaration" | "variable_declarator" | "assignment" | "short_var_declaration"
        )
        && let Some((_, field)) = spec.binders.iter().find(|(k, _)| *k == kind)
        && let Some(target) = node.child_by_field_name(field)
        && let Some(value) = node
            .child_by_field_name("value")
            .or_else(|| node.child_by_field_name("right"))
        && (root_ident(unwrap_iterable(comprehended(value), source), source)
            .is_some_and(|r| scope.bound.contains(&r))
            || keyed_by(value, source, spec, &scope.bound))
    {
        pattern_names(target, source, spec.idents, &mut scope.bound);
    }
    if kind == "as_pattern"
        && !scope.bound.is_empty()
        && let (Some(value), Some(target)) =
            (node.named_child(0), node.child_by_field_name("alias"))
        && root_ident(unwrap_iterable(value, source), source)
            .is_some_and(|r| scope.bound.contains(&r))
    {
        pattern_names(target, source, spec.idents, &mut scope.bound);
    }
    if let Some((_, field)) = spec.binders.iter().find(|(k, _)| *k == kind)
        && let Some(target) = node.child_by_field_name(field)
        && let Some(value) = node
            .child_by_field_name("value")
            .or_else(|| node.child_by_field_name("right"))
    {
        track_local(kind, target, value, source, spec, scope);
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
    if spec.early_exits.contains(&kind) && Some(node.id()) != scope.last_stmt {
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
        let top = chain_top(node, spec);
        let flow = scope
            .flows
            .entry(top.id())
            .or_insert_with(|| data_flow(top, source, spec));
        let per_item = st.loop_depth > 0
            && per_item(node, info.receiver.as_deref(), source, spec, &scope.bound);
        let mut call = site(callee, info.receiver, info.string_args, at, awaited, st);
        call.binds = flow.binds.clone();
        call.uses = flow.uses.clone();
        call.mutates = flow.mutates.clone();
        call.per_item = per_item;
        call.guards = scope.guards.clone();
        call.on_part = node
            .child_by_field_name("arguments")
            .and_then(|a| a.named_child(0))
            .is_some_and(|a| {
                PART_KINDS.contains(&a.kind())
                    || (spec.idents.contains(&a.kind())
                        && scope.parts.iter().any(|p| p == text(a, source)))
            });
        out.calls.push(call);
        if let Some(expand) = spec.expand {
            for mut c in expand(node, source) {
                c.loop_depth += st.loop_depth;
                c.conditional |= st.conditional;
                c.in_closure |= st.in_closure;
                c.awaited |= awaited;
                c.binds = flow.binds.clone();
                c.uses = flow.uses.clone();
                c.mutates = flow.mutates.clone();
                out.calls.push(c);
            }
        }
        if let Some(args) = node.child_by_field_name("arguments") {
            emit_refs(&named_children(args), source, spec, st, out, scope.is_ref);
        }
    }
    if spec.ref_containers.contains(&kind) {
        let elements = match node.child_by_field_name("value") {
            Some(v) => vec![v],
            None => named_children(node),
        };
        emit_refs(&elements, source, spec, st, out, scope.is_ref);
    }
    let is_cond = spec.conditionals.contains(&kind);
    let guard = node.child_by_field_name("condition").map(|n| n.id());
    let tested = (is_cond || spec.switches.contains(&kind))
        .then(|| {
            node.child_by_field_name("condition")
                .or_else(|| node.child_by_field_name("value"))
        })
        .flatten();
    let guards_before = scope.guards.len();
    for child in named_children(node) {
        let mut cst = st;
        if is_cond && guard != Some(child.id()) {
            cst.conditional = true;
        }
        let in_branch = tested.is_some_and(|t| t.id() != child.id());
        if in_branch && scope.guards.len() == guards_before {
            let mut names = Vec::new();
            if let Some(t) = tested {
                idents(t, source, spec, &mut names);
            }
            scope.guards.extend(names);
        }
        if !in_branch {
            scope.guards.truncate(guards_before);
        }
        walk(child, source, spec, cst, out, scope);
    }
    scope.guards.truncate(guards_before);
    if spec.loops.contains(&kind) {
        scope.bound.truncate(bound_before);
        scope.iterated.truncate(iterated_before);
    }
}

/// Expressions that take a part of a value: a field, a member, an index or a slice.
const PART_KINDS: &[&str] = &[
    "field_expression",
    "member_expression",
    "attribute",
    "subscript",
    "subscript_expression",
    "index_expression",
    "selector_expression",
    "slice_expression",
];

/// A call whose arguments and receiver read only names bound by the enclosing loops.
fn per_item(
    node: Node,
    receiver: Option<&str>,
    source: &str,
    spec: &BodySpec,
    bound: &[String],
) -> bool {
    let mut names = Vec::new();
    if let Some(args) = node.child_by_field_name("arguments") {
        idents(args, source, spec, &mut names);
    }
    if let Some(root) = receiver
        .filter(|r| *r != EXPR_RECEIVER)
        .and_then(|r| r.split(['.', ':']).find(|p| !p.is_empty()))
    {
        names.push(root.to_owned());
    }
    names.retain(|n| !NOT_A_FUNCTION.contains(&n.as_str()));
    !names.is_empty() && names.iter().all(|n| bound.contains(n))
}

/// The variable pattern and the iterated expression of a loop, when it has them:
/// `for p in it`, `for (const p of it)`, `[.. for p in it]`, `while let p = it`, `for p := range it`.
fn loop_parts(node: Node) -> (Option<Node>, Option<Node>) {
    let clause = if node.kind().ends_with("comprehension") || node.kind() == "generator_expression"
    {
        child_of_kind(node, "for_in_clause")
    } else if let Some(range) = child_of_kind(node, "range_clause") {
        Some(range)
    } else if let Some(cond) = node
        .child_by_field_name("condition")
        .filter(|c| c.kind() == "let_condition")
    {
        Some(cond)
    } else {
        Some(node)
    };
    let Some(clause) = clause else {
        return (None, None);
    };
    let pattern = clause
        .child_by_field_name("pattern")
        .or_else(|| clause.child_by_field_name("left"));
    let iterable = clause
        .child_by_field_name("value")
        .or_else(|| clause.child_by_field_name("right"));
    (pattern, iterable)
}

/// Loops that do not scale with the input, so they add no loop depth: over a literal list, a
/// constant table or a small literal range; retries; paced, polling and event loops that wait
/// between iterations; loops that stop at a constant cap (`take(K)`, `[:K]`, `if n >= CAP:
/// break`); a window around a point (`range(x - r, x + r + 1)`); and an inner loop over a part
/// of an enclosing loop's item (a `group.items` walk, or a lookup keyed by it like
/// `groups_of(c.id)`, visits each item once).
fn bounded_loop(
    node: Node,
    pattern: Option<Node>,
    iterable: Option<Node>,
    source: &str,
    spec: &BodySpec,
    scope: &Scope,
) -> bool {
    let bound = scope.bound.as_slice();
    let body = node
        .child_by_field_name("body")
        .map_or("", |b| text(b, source));
    if node
        .child_by_field_name("body")
        .is_some_and(|b| cap_break(b, source, spec))
    {
        return true;
    }
    if [
        "sleep(",
        "sleep_until(",
        ".tick()",
        "select!",
        "setTimeout(",
    ]
    .iter()
    .any(|p| body.contains(p))
    {
        return true;
    }
    let pattern_text = pattern.map_or("", |p| text(p, source));
    if retry_named(pattern_text) {
        return true;
    }
    if let Some(cond) = node
        .child_by_field_name("condition")
        .or_else(|| {
            child_of_kind(node, "for_clause").and_then(|c| c.child_by_field_name("condition"))
        })
        .filter(|c| c.kind() != "let_condition")
    {
        if c_style_bounded(node, cond, source, scope) {
            return true;
        }
        let cond = text(cond, source);
        if retry_named(cond) || small_bound(cond) {
            return true;
        }
    }
    let Some(it) = iterable else {
        return false;
    };
    let it_text = text(it, source);
    if [".recv()", ".next().await", ".accept()", ".message().await"]
        .iter()
        .any(|p| it_text.contains(p))
        || CAPPED.is_match(&strip_ws(it_text))
    {
        return true;
    }
    let base = unwrap_iterable(it, source);
    if constant_iterable(base, source)
        || (spec.idents.contains(&base.kind())
            && scope.small.iter().any(|s| s == text(base, source)))
        || retry_named(last_segment(&strip_ws(text(base, source))).as_str())
        || window(base, source)
        || reach_range(base, source, &scope.near)
        || (!is_range(base, source) && keyed_by(base, source, spec, bound))
        || part_indices(base, source, spec, bound)
    {
        return true;
    }
    !is_range(base, source)
        && root_ident(base, source).is_some_and(|r| r != "self" && bound.contains(&r))
}

/// An `if` of the loop's own body (not of a nested loop or closure) that breaks at a
/// constant cap.
fn cap_break(node: Node, source: &str, spec: &BodySpec) -> bool {
    named_children(node).into_iter().any(|c| {
        if spec.loops.contains(&c.kind()) || spec.closures.contains(&c.kind()) {
            return false;
        }
        (c.kind().starts_with("if") && CAP_BREAK.is_match(text(c, source)))
            || cap_break(c, source, spec)
    })
}

/// `if len(found) >= CAP: break`, `if n > 10 { break; }`.
static CAP_BREAK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"if\s[^\n:{]*(>=|>|==)\s*([A-Z][A-Z0-9_]*|\d+)\s*[:{]\s*(\{\s*)?break\b")
        .expect("valid regex")
});

/// `.take(10)`, `.take(MAX)`, `[:5]`, `[..LIMIT]`, `.slice(0, 20)`.
static CAPPED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\.take\(([A-Z][A-Z0-9_]*|\d+)\)|\[(0?:|\.\.=?)([A-Z][A-Z0-9_]*|\d+)\]|\.slice\(0,([A-Z][A-Z0-9_]*|\d+)\)")
        .expect("valid regex")
});

/// The two ends of `range(a, b)` / `a..b`, as written without whitespace.
fn range_ends(node: Node, source: &str) -> Option<(String, String)> {
    if !is_range(node, source) {
        return None;
    }
    let ends: Vec<String> = match node.child_by_field_name("arguments") {
        Some(args) => named_children(args),
        None => named_children(node),
    }
    .into_iter()
    .map(|e| strip_ws(text(e, source)))
    .collect();
    match <[String; 2]>::try_from(ends) {
        Ok([start, end]) => Some((start, end)),
        Err(_) => None,
    }
}

/// `range(x - r, x + r + 1)`, `range(-r, r + 1)`, `x - r..=x + r`: a window of fixed reach
/// around a point.
fn window(node: Node, source: &str) -> bool {
    let Some((start, end)) = range_ends(node, source) else {
        return false;
    };
    let Some((at, reach)) = start.rsplit_once('-') else {
        return false;
    };
    let at = if at.is_empty() {
        at.to_owned()
    } else {
        format!("{at}+")
    };
    !reach.is_empty() && (end == format!("{at}{reach}") || end == format!("{at}{reach}+1"))
}

/// `range(at, at + GAP)`, `range(at, limit + 1)` with `limit = min(n, at + GAP)`: a constant
/// reach past the start.
fn reach_range(node: Node, source: &str, near: &HashMap<String, String>) -> bool {
    range_ends(node, source).is_some_and(|(start, end)| reaches(&start, &end, near))
}

fn reaches(start: &str, end: &str, near: &HashMap<String, String>) -> bool {
    let end = end
        .strip_suffix("+1")
        .or_else(|| end.strip_suffix("-1"))
        .unwrap_or(end);
    anchor_of(end).or_else(|| near.get(end).cloned()).as_deref() == Some(start)
}

/// `at + GAP`, `min(n, at + GAP)`, `Math.min(n - 1, at + 3)`, `(at + K).min(n)` → `at`.
fn anchor_of(t: &str) -> Option<String> {
    let t = t
        .strip_suffix("+1")
        .or_else(|| t.strip_suffix("-1"))
        .unwrap_or(t);
    let args: Vec<&str> = if let Some(inner) = ["min(", "Math.min(", "cmp::min(", "std::cmp::min("]
        .iter()
        .find_map(|p| t.strip_prefix(p))
        .and_then(|r| r.strip_suffix(')'))
    {
        top_level_args(inner)
    } else if let Some((recv, _)) = t.split_once(".min(") {
        vec![recv]
    } else {
        vec![t]
    };
    args.into_iter().find_map(|a| {
        let a = a.trim_start_matches('(').trim_end_matches(')');
        REACH.captures(a).map(|c| c[1].to_owned())
    })
}

/// [`anchor_of`], also through a choice between the point and a reach past it
/// (`at if k == 0 else min(n, at + GAP)`).
fn anchor_value(node: Node, source: &str) -> Option<String> {
    let branches = match node.kind() {
        "conditional_expression" => {
            let items = named_children(node);
            (items.len() == 3).then(|| [items[0], items[2]])
        }
        "ternary_expression" => node
            .child_by_field_name("consequence")
            .zip(node.child_by_field_name("alternative"))
            .map(|(a, b)| [a, b]),
        _ => return anchor_of(&strip_ws(text(node, source))),
    }?;
    let [a, b] = branches.map(|n| strip_ws(text(n, source)));
    match (anchor_of(&a), anchor_of(&b)) {
        (Some(x), Some(y)) if x == y => Some(x),
        (Some(x), None) if x == b => Some(x),
        (None, Some(y)) if y == a => Some(y),
        _ => None,
    }
}

/// `x + 3`, `at + MAX_GAP`.
static REACH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^([A-Za-z_][\w.]*)\+([A-Z][A-Z0-9_]*|\d+)$").expect("valid regex")
});

fn top_level_args(t: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut depth, mut from) = (0i32, 0);
    for (i, c) in t.char_indices() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            ',' if depth == 0 => {
                out.push(&t[from..i]);
                from = i + 1;
            }
            _ => {}
        }
    }
    out.push(&t[from..]);
    out
}

/// A C-style loop that walks a part of an enclosing loop's item (`c < buffer.channels`,
/// `i < row.len()`) or a constant reach past its start (`i = at; i <= limit`).
fn c_style_bounded(node: Node, cond: Node, source: &str, scope: &Scope) -> bool {
    let cond = if cond.kind() == "expression_statement" {
        match cond.named_child(0) {
            Some(c) => c,
            None => return false,
        }
    } else {
        cond
    };
    let (Some(var), Some(op), Some(rhs)) = (
        cond.child_by_field_name("left"),
        cond.child_by_field_name("operator"),
        cond.child_by_field_name("right"),
    ) else {
        return false;
    };
    if !matches!(text(op, source), "<" | "<=") {
        return false;
    }
    if (PART_KINDS.contains(&rhs.kind()) || rhs.kind().contains("call"))
        && root_ident(rhs, source)
            .is_some_and(|r| r != "self" && r != "this" && scope.bound.contains(&r))
    {
        return true;
    }
    let init = node.child_by_field_name("initializer").or_else(|| {
        child_of_kind(node, "for_clause").and_then(|c| c.child_by_field_name("initializer"))
    });
    init.and_then(|i| initial_value(i, text(var, source), source))
        .is_some_and(|start| reaches(&start, &strip_ws(text(rhs, source)), &scope.near))
}

/// The value `name` starts at in a loop initializer (`let i = at`, `i := at`).
fn initial_value(node: Node, name: &str, source: &str) -> Option<String> {
    let target = node
        .child_by_field_name("name")
        .or_else(|| node.child_by_field_name("left"));
    let value = node
        .child_by_field_name("value")
        .or_else(|| node.child_by_field_name("right"));
    if let (Some(t), Some(v)) = (target, value)
        && strip_ws(text(t, source)) == name
    {
        return Some(strip_ws(text(v, source)));
    }
    named_children(node)
        .into_iter()
        .find_map(|c| initial_value(c, name, source))
}

/// Records what a binding makes of its names: a small collection, a fixed reach past a point,
/// or a part of a value.
fn track_local(
    kind: &str,
    target: Node,
    value: Node,
    source: &str,
    spec: &BodySpec,
    scope: &mut Scope,
) {
    let mut names = Vec::new();
    pattern_names(target, source, spec.idents, &mut names);
    let plain = !kind.contains("augmented") && !kind.contains("compound");
    let small = plain && small_value(value, source);
    let part = plain && part_value(value, source);
    let anchor = (plain && names.len() == 1)
        .then(|| anchor_value(value, source))
        .flatten();
    for n in names {
        let small = small && !scope.grown.contains(&n);
        scope.small.retain(|s| *s != n);
        scope.parts.retain(|s| *s != n);
        if small {
            scope.small.push(n.clone());
        }
        if part {
            scope.parts.push(n.clone());
        }
        match &anchor {
            Some(a) => scope.near.insert(n, a.clone()),
            None => scope.near.remove(&n),
        };
    }
}

const LITERAL_KINDS: &[&str] = &[
    "array_expression",
    "tuple_expression",
    "array",
    "list",
    "tuple",
    "set",
];

/// A literal of at most [`SMALL_BOUND`] items (or a concatenation or choice of such), a
/// stencil collected from a window, or a comprehension over windows and constants.
fn small_value(node: Node, source: &str) -> bool {
    small_or_empty(node, source) && !named_children(node).is_empty()
}

fn small_or_empty(node: Node, source: &str) -> bool {
    let items = named_children(node);
    match node.kind() {
        "parenthesized_expression" => items.len() == 1 && small_or_empty(items[0], source),
        k if LITERAL_KINDS.contains(&k) => {
            items.len() <= SMALL_BOUND as usize
                && node.child_by_field_name("length").is_none()
                && !items.iter().any(|i| {
                    matches!(
                        i.kind(),
                        "list_splat" | "spread_element" | "dictionary_splat"
                    )
                })
        }
        "binary_operator" | "binary_expression" => {
            node.child_by_field_name("operator")
                .is_some_and(|o| text(o, source) == "+")
                && items.len() == 2
                && items.iter().all(|i| small_or_empty(*i, source))
        }
        "conditional_expression" => {
            items.len() == 3 && small_or_empty(items[0], source) && small_or_empty(items[2], source)
        }
        "ternary_expression" => ["consequence", "alternative"].iter().all(|f| {
            node.child_by_field_name(f)
                .is_some_and(|b| small_or_empty(b, source))
        }),
        k if k.ends_with("comprehension") || k == "generator_expression" => {
            let clauses: Vec<Node> = items
                .into_iter()
                .filter(|c| c.kind() == "for_in_clause")
                .collect();
            !clauses.is_empty()
                && clauses.iter().all(|c| {
                    c.child_by_field_name("right").is_some_and(|it| {
                        let it = unwrap_iterable(it, source);
                        window(it, source) || constant_iterable(it, source)
                    })
                })
        }
        k if k.contains("call") => {
            let base = chain_base(node);
            base.id() != node.id() && window(base, source)
        }
        _ => false,
    }
}

/// The value a method chain starts from: `(-r..=r).flat_map(..).collect()` → `-r..=r`.
fn chain_base(mut node: Node) -> Node {
    loop {
        let next = match node.kind() {
            "parenthesized_expression" | "reference_expression" | "await_expression" => {
                named_children(node).pop()
            }
            k if k.contains("call") => node.child_by_field_name("function").and_then(|f| {
                f.child_by_field_name("value")
                    .or_else(|| f.child_by_field_name("object"))
            }),
            _ => None,
        };
        match next {
            Some(n) => node = n,
            None => return node,
        }
    }
}

/// `xs[:mid]`, `&xs[..mid]`, `(xs[:m], xs[m:])`, `xs.split_at(mid)`: a part of a value.
fn part_value(node: Node, source: &str) -> bool {
    match node.kind() {
        "reference_expression" | "parenthesized_expression" => named_children(node)
            .pop()
            .is_some_and(|n| part_value(n, source)),
        "subscript" | "subscript_expression" | "index_expression" | "slice_expression" => true,
        "expression_list" | "tuple" | "tuple_expression" => {
            let items = named_children(node);
            !items.is_empty() && items.iter().all(|i| part_value(*i, source))
        }
        k if k.contains("call") => node.child_by_field_name("function").is_some_and(|f| {
            matches!(
                last_segment(&strip_ws(text(f, source))).as_str(),
                "split_at" | "split_at_mut" | "split_first" | "split_last"
            )
        }),
        _ => false,
    }
}

/// Parameters defaulting to a short literal (`rings=(35.0, 50.0, 70.0)`) of the function
/// whose body starts at `first`.
fn literal_defaults(first: Node, source: &str, spec: &BodySpec) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = first.parent();
    for _ in 0..3 {
        let Some(n) = cur else { break };
        if let Some(params) = n.child_by_field_name("parameters") {
            for p in named_children(params) {
                if let (Some(v), Some(name)) = (
                    p.child_by_field_name("value"),
                    p.child_by_field_name("name")
                        .or_else(|| p.child_by_field_name("pattern")),
                ) && small_value(v, source)
                {
                    pattern_names(name, source, spec.idents, &mut out);
                }
            }
            break;
        }
        cur = n.parent();
    }
    out
}

/// Names a body grows in place: receivers of `append`/`push`/`extend`/`insert`/`add`, and
/// targets of `+=`.
fn grown(node: Node, source: &str, out: &mut Vec<String>) {
    let k = node.kind();
    if k.contains("call")
        && let Some(f) = node.child_by_field_name("function")
        && let Some(obj) = f
            .child_by_field_name("value")
            .or_else(|| f.child_by_field_name("object"))
            .or_else(|| f.child_by_field_name("operand"))
        && matches!(
            last_segment(&strip_ws(text(f, source))).as_str(),
            "append"
                | "appendleft"
                | "push"
                | "push_back"
                | "push_front"
                | "unshift"
                | "extend"
                | "insert"
                | "add"
                | "update"
        )
        && let Some(r) = root_ident(obj, source)
    {
        out.push(r);
    }
    if (k.contains("augmented") || k.contains("compound"))
        && let Some(t) = node.child_by_field_name("left")
        && let Some(r) = root_ident(t, source)
    {
        out.push(r);
    }
    for c in named_children(node) {
        grown(c, source, out);
    }
}

/// The index of `m[k]`.
fn subscript_index(node: Node) -> Option<Node> {
    match node.kind() {
        "subscript" => node.child_by_field_name("subscript"),
        "subscript_expression" | "index_expression" => node
            .child_by_field_name("index")
            .or_else(|| node.named_child(1)),
        _ => None,
    }
}

/// The iterated expression of a comprehension, else the expression itself.
fn comprehended(node: Node) -> Node {
    if node.kind().ends_with("comprehension") || node.kind() == "generator_expression" {
        loop_parts(node).1.unwrap_or(node)
    } else {
        node
    }
}

/// `range(len(row))`, `range(min(len(a), len(b)))`, `0..item.len()`: the indices of a part of
/// an enclosing loop's item.
fn part_indices(node: Node, source: &str, spec: &BodySpec, bound: &[String]) -> bool {
    if !is_range(node, source) {
        return false;
    }
    let t = strip_ws(text(node, source));
    let mut names = Vec::new();
    idents(node, source, spec, &mut names);
    names.retain(|n| n != "range" && n != "len" && n != "min");
    !names.is_empty()
        && names.iter().all(|n| {
            bound.contains(n)
                && (t.contains(&format!("len({n})")) || t.contains(&format!("{n}.len()")))
        })
}

/// A call outside any closure whose arguments read only names the enclosing loops bound, or a
/// lookup keyed by them: `ctx.groups_of(c.id)`, `repo.copies(*id, hash).await?`, `m[k]`.
fn keyed_by(node: Node, source: &str, spec: &BodySpec, bound: &[String]) -> bool {
    if spec.closures.contains(&node.kind()) {
        return false;
    }
    if let Some(index) =
        subscript_index(node).filter(|i| !matches!(i.kind(), "slice" | "range_expression"))
    {
        let mut names = Vec::new();
        idents(index, source, spec, &mut names);
        if !names.is_empty() && names.iter().all(|n| bound.contains(n)) {
            return true;
        }
    }
    if (node.kind().contains("call") || node.kind() == "new_expression")
        && let Some(args) = node.child_by_field_name("arguments")
        && !named_children(args)
            .iter()
            .any(|a| spec.closures.contains(&a.kind()))
    {
        let mut names = Vec::new();
        idents(args, source, spec, &mut names);
        names.retain(|n| !NOT_A_FUNCTION.contains(&n.as_str()));
        if !names.is_empty() && names.iter().all(|n| bound.contains(n)) {
            return true;
        }
    }
    named_children(node)
        .into_iter()
        .any(|c| keyed_by(c, source, spec, bound))
}

fn retry_named(t: &str) -> bool {
    ident_words(t).iter().any(|w| {
        matches!(
            w.as_str(),
            "attempt" | "attempts" | "retry" | "retries" | "tries" | "backoff"
        )
    })
}

/// `i < 3`, `n <= 10`: a C-style loop bounded by a small literal.
fn small_bound(cond: &str) -> bool {
    let cond = cond.trim().trim_end_matches(';');
    cond.split_once('<')
        .map(|(_, rhs)| rhs.trim_start_matches('=').trim().trim_end_matches(')'))
        .and_then(|rhs| rhs.parse::<u64>().ok())
        .is_some_and(|n| n <= SMALL_BOUND)
}

const SMALL_BOUND: u64 = 16;

/// `&x`, `x.iter()`, `x.items()`, `enumerate(x)`, `Object.entries(x)` → `x`.
fn unwrap_iterable<'t>(mut node: Node<'t>, source: &str) -> Node<'t> {
    const ADAPTERS: &[&str] = &[
        "iter",
        "into_iter",
        "iter_mut",
        "items",
        "keys",
        "values",
        "entries",
        "enumerate",
        "copied",
        "cloned",
        "sorted",
        "reversed",
        "list",
        "tuple",
    ];
    loop {
        match node.kind() {
            "reference_expression"
            | "parenthesized_expression"
            | "unary_expression"
            | "await_expression"
            | "await" => {
                let Some(inner) = named_children(node).pop() else {
                    return node;
                };
                node = inner;
            }
            k if k.contains("call") => {
                let Some(f) = node.child_by_field_name("function") else {
                    return node;
                };
                let name = last_segment(&strip_ws(text(f, source)));
                if !ADAPTERS.contains(&name.as_str()) {
                    return node;
                }
                let receiver = f
                    .child_by_field_name("value")
                    .or_else(|| f.child_by_field_name("object"))
                    .filter(|r| text(*r, source) != "Object");
                let next = receiver.or_else(|| {
                    node.child_by_field_name("arguments").and_then(|a| {
                        if a.kind() == "generator_expression" {
                            Some(a)
                        } else {
                            a.named_child(0)
                        }
                    })
                });
                match next {
                    Some(n) => node = n,
                    None => return node,
                }
            }
            _ => return node,
        }
    }
}

fn constant_iterable(node: Node, source: &str) -> bool {
    match node.kind() {
        "array_expression" | "tuple_expression" | "array" | "list" | "tuple" | "set" => true,
        "identifier"
        | "scoped_identifier"
        | "field_expression"
        | "attribute"
        | "member_expression"
        | "selector_expression" => {
            let last = last_segment(&strip_ws(text(node, source)));
            last.len() > 1
                && last.chars().any(|c| c.is_ascii_uppercase())
                && last
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        }
        "range_expression" => {
            let ends: Vec<Node> = named_children(node);
            !ends.is_empty()
                && ends.iter().all(|e| e.kind() == "integer_literal")
                && ends
                    .last()
                    .and_then(|e| text(*e, source).parse::<u64>().ok())
                    .is_some_and(|n| n <= SMALL_BOUND)
        }
        "call"
            if node
                .child_by_field_name("function")
                .is_some_and(|f| text(f, source) == "range") =>
        {
            let args: Vec<Node> = node
                .child_by_field_name("arguments")
                .map(named_children)
                .unwrap_or_default();
            !args.is_empty()
                && args.iter().all(|a| a.kind() == "integer")
                && args
                    .iter()
                    .filter_map(|a| text(*a, source).parse::<u64>().ok())
                    .max()
                    .is_some_and(|n| n <= SMALL_BOUND)
        }
        _ => false,
    }
}

fn is_range(node: Node, source: &str) -> bool {
    node.kind() == "range_expression"
        || (node.kind() == "call"
            && node
                .child_by_field_name("function")
                .is_some_and(|f| text(f, source) == "range"))
}

/// The variable an expression reads from: `m.tracks` → `m`, `row["cells"]` → `row`,
/// `read_dir(&dir)` → `dir`, `line.split(';')` → `line`.
fn root_ident(mut node: Node, source: &str) -> Option<String> {
    for _ in 0..16 {
        match node.kind() {
            "identifier" => return Some(text(node, source).to_owned()),
            k if k.contains("call") => {
                let f = node.child_by_field_name("function")?;
                node = match f
                    .child_by_field_name("value")
                    .or_else(|| f.child_by_field_name("object"))
                    .or_else(|| f.child_by_field_name("operand"))
                {
                    Some(receiver) => receiver,
                    None => node.child_by_field_name("arguments")?.named_child(0)?,
                };
            }
            _ => {
                node = node
                    .child_by_field_name("value")
                    .or_else(|| node.child_by_field_name("object"))
                    .or_else(|| node.child_by_field_name("operand"))
                    .or_else(|| node.named_child(0))?;
            }
        }
    }
    None
}

/// Identifiers bound by a loop pattern: `(k, group)` → `k`, `group`.
fn pattern_names(node: Node, source: &str, idents: &[&str], out: &mut Vec<String>) {
    if idents.contains(&node.kind()) {
        out.push(text(node, source).to_owned());
        return;
    }
    for c in named_children(node) {
        pattern_names(c, source, idents, out);
    }
}

/// Lowercase words of identifiers: `max_attempts` → `max`, `attempts`; `retryDelay` → `retry`,
/// `delay`.
fn ident_words(t: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![String::new()];
    let mut prev_lower = false;
    for c in t.chars() {
        if !c.is_alphanumeric() {
            out.push(String::new());
            prev_lower = false;
            continue;
        }
        if c.is_uppercase() && prev_lower {
            out.push(String::new());
        }
        prev_lower = c.is_lowercase() || c.is_ascii_digit();
        if let Some(w) = out.last_mut() {
            w.extend(c.to_lowercase());
        }
    }
    out.retain(|w| !w.is_empty());
    out
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
            out.calls.push(CallSite {
                reference: true,
                ..site(t, None, vec![], *n, false, st)
            });
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
        binds: vec![],
        uses: vec![],
        mutates: vec![],
        reference: false,
        per_item: false,
        concurrent: false,
        on_part: false,
        guards: vec![],
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

/// Node kinds of an `if` / `else if` / `else` chain.
pub struct ChainSpec<'a> {
    pub ifs: &'a [&'a str],
    /// `elif_clause`: carries its own `condition` and `consequence`.
    pub elifs: &'a [&'a str],
    /// `else_clause`: wraps a block or a further `if`.
    pub elses: &'a [&'a str],
}

/// `(condition, body)` per arm of the chain starting at `node`, and the final `else` body.
pub fn if_chain<'t>(
    node: Node<'t>,
    spec: &ChainSpec,
) -> (Vec<(Node<'t>, Node<'t>)>, Option<Node<'t>>) {
    let mut arms = Vec::new();
    let mut else_body = None;
    let mut cur = node;
    'outer: loop {
        if let (Some(c), Some(b)) = (
            cur.child_by_field_name("condition"),
            cur.child_by_field_name("consequence"),
        ) {
            arms.push((c, b));
        }
        let mut walker = cur.walk();
        let alternatives: Vec<Node<'t>> = cur
            .children_by_field_name("alternative", &mut walker)
            .collect();
        for alt in alternatives {
            let k = alt.kind();
            if spec.ifs.contains(&k) {
                cur = alt;
                continue 'outer;
            }
            if spec.elifs.contains(&k) {
                if let (Some(c), Some(b)) = (
                    alt.child_by_field_name("condition"),
                    alt.child_by_field_name("consequence"),
                ) {
                    arms.push((c, b));
                }
                continue;
            }
            let inner = if spec.elses.contains(&k) {
                alt.child_by_field_name("body")
                    .or_else(|| named_children(alt).into_iter().next())
            } else {
                Some(alt)
            };
            match inner {
                Some(i) if spec.ifs.contains(&i.kind()) => {
                    cur = i;
                    continue 'outer;
                }
                other => else_body = other,
            }
        }
        break;
    }
    (arms, else_body)
}

/// One equality test of a subject against values: `x == A`, `x === 'a' || x === 'b'`,
/// `x in ('a', 'b')`, `matches!(x, A | B)`.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Comparison {
    pub subject: String,
    pub hint: Option<String>,
    pub values: Vec<String>,
}

impl Comparison {
    pub fn new(subject: &str, values: Vec<Handled>) -> Option<Self> {
        if values.is_empty() {
            return None;
        }
        let mut out = Self {
            subject: strip_ws(subject),
            ..Self::default()
        };
        out.extend(values);
        Some(out)
    }

    pub fn extend(&mut self, values: Vec<Handled>) {
        for v in values {
            if self.hint.is_none() {
                self.hint = v.hint;
            }
            if !self.values.contains(&v.value) {
                self.values.push(v.value);
            }
        }
    }

    /// Joins two tests on the same subject (`a == X || a == Y`).
    pub fn merge(a: Option<Self>, b: Option<Self>) -> Option<Self> {
        let (mut a, b) = (a?, b?);
        if a.subject != b.subject {
            return None;
        }
        a.extend(
            b.values
                .into_iter()
                .map(|value| Handled {
                    hint: b.hint.clone(),
                    value,
                })
                .collect(),
        );
        Some(a)
    }
}

/// A value a branch handles, with the enum or namespace it was qualified with.
#[derive(Debug, Clone, PartialEq)]
pub struct Handled {
    pub hint: Option<String>,
    pub value: String,
}

impl Handled {
    pub fn literal(value: String) -> Self {
        Self { hint: None, value }
    }

    /// `Status::Active` / `Status.Active` → hint `Status`, value `Active`; `Self::A` and
    /// lowercase namespaces (`self.kind`, `opts.mode`) are not members of a domain.
    pub fn member(path: &str, sep: &str) -> Option<Self> {
        let path = strip_ws(path);
        let (ns, value) = path.rsplit_once(sep)?;
        let ns = ns.rsplit(sep).next().unwrap_or(ns);
        let typed = |s: &str| s.starts_with(|c: char| c.is_ascii_uppercase());
        if !typed(ns) || value.is_empty() || value.starts_with(|c: char| c.is_ascii_digit()) {
            return None;
        }
        let hint = (!matches!(ns, "Self" | "Some" | "Ok" | "Err")).then(|| ns.to_owned());
        if hint.is_none() && ns != "Self" {
            return None;
        }
        Some(Self {
            hint,
            value: value.to_owned(),
        })
    }
}

const SWITCH_VALUE_SUBJECTS: usize = 80;

/// Accumulates the switch-like sites of one function body.
#[derive(Default)]
pub struct Sites {
    pub out: Vec<Discrimination>,
    seen_ifs: Vec<usize>,
}

impl Sites {
    pub fn push(&mut self, line: u32, cmp: Comparison, default: DefaultKind) {
        if cmp.values.is_empty() {
            return;
        }
        let mut subject = cmp.subject;
        subject.truncate(SWITCH_VALUE_SUBJECTS);
        self.out.push(Discrimination {
            subject,
            domain_hint: cmp.hint,
            handled: cmp.values,
            line,
            default,
        });
    }

    /// Records an `if` chain whose every arm compares the same subject; nested `if`s already
    /// consumed as `else if` arms are skipped when met again by the walker.
    pub fn if_chain(
        &mut self,
        node: Node,
        source: &str,
        spec: &ChainSpec,
        compare: &dyn Fn(Node, &str) -> Option<Comparison>,
        default_body: &dyn Fn(Node, &str) -> String,
    ) {
        if self.seen_ifs.contains(&node.id()) {
            return;
        }
        let (arms, else_body) = if_chain(node, spec);
        let mut cmp: Option<Comparison> = None;
        for (i, (cond, _)) in arms.iter().enumerate() {
            if i > 0
                && let Some(p) = cond.parent()
            {
                self.seen_ifs.push(p.id());
            }
            let c = compare(*cond, source);
            cmp = if i == 0 { c } else { Comparison::merge(cmp, c) };
            if cmp.is_none() {
                return;
            }
        }
        let Some(cmp) = cmp else { return };
        if arms.len() < 2 && cmp.values.len() < 2 {
            return;
        }
        let default = match else_body {
            Some(b) => classify_default(&default_body(b, source)),
            None => DefaultKind::None,
        };
        self.push(line(node), cmp, default);
    }
}

const THROW_PREFIXES: &[&str] = &[
    "throw ",
    "throw(",
    "raise ",
    "raise(",
    "raise",
    "panic!",
    "panic(",
    "unreachable!",
    "todo!",
    "unimplemented!",
    "bail!",
    "anyhow::bail!",
    "return Err(",
    "Err(",
    "log.Fatal",
    "process.exit(",
    "os.Exit(",
    "sys.exit(",
    "assert ",
    "assert(",
];

const EMPTY_RESULTS: &[&str] = &[
    "",
    "[]",
    "{}",
    "null",
    "undefined",
    "None",
    "nil",
    "nil,nil",
    "()",
    "\"\"",
    "''",
    "``",
    "false",
    "Vec::new()",
    "vec![]",
    "HashMap::new()",
    "String::new()",
    "Ok(())",
    "Ok(None)",
    "Ok(vec![])",
    "Default::default()",
    "0",
];

/// See [`DefaultKind`] for the rules. `body` is the catch-all's source text.
pub fn classify_default(body: &str) -> DefaultKind {
    let mut t = collapse_ws(body);
    loop {
        let trimmed = t.trim_matches([' ', ';']);
        match trimmed.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
            Some(inner) => t = inner.trim().to_owned(),
            None => {
                t = trimmed.to_owned();
                break;
            }
        }
    }
    let stripped = strip_ws(&t);
    if stripped.is_empty() || matches!(stripped.as_str(), "break" | "continue" | "pass" | "return")
    {
        return DefaultKind::ReturnsEmpty;
    }
    if THROW_PREFIXES.iter().any(|p| t.starts_with(p))
        || t.strip_prefix("return ")
            .is_some_and(|r| THROW_PREFIXES.iter().any(|p| r.starts_with(p)))
        || never_call(&t)
    {
        return DefaultKind::Throws;
    }
    let value = stripped
        .strip_prefix("return")
        .map(|r| r.trim_end_matches(';'))
        .unwrap_or(stripped.as_str())
        .trim_end_matches(';');
    let value = value.trim_end_matches("break").trim_end_matches(';');
    if EMPTY_RESULTS.contains(&value) {
        return DefaultKind::ReturnsEmpty;
    }
    DefaultKind::Other
}

/// A call whose name says the branch is unreachable (`assertNever(x)`, `exhaustiveCheck(x)`).
fn never_call(t: &str) -> bool {
    let Some(paren) = t.find('(') else {
        return false;
    };
    let name = last_segment(t[..paren].trim().rsplit(' ').next().unwrap_or("")).to_lowercase();
    name.contains("never")
        || name.contains("unreachable")
        || name.contains("exhaustive")
        || name.contains("invalid")
        || name.contains("unexpected")
        || name.contains("unknown")
}
