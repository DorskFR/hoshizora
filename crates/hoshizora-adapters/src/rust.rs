use hoshizora_model::{
    CallSite, DefaultKind, Domain, DomainValue, FileFacts, Import, Language, Param, Symbol,
    SymbolKind, Visibility,
};
use tree_sitter::Node;

use crate::Adapter;
use crate::common::{
    self, BodySpec, CallInfo, ChainSpec, Comparison, Handled, Sites, all_children, child_of_kind,
    clean_comment_lines, collapse_ws, field_text, has_child_kind, header, last_segment,
    named_children, preceding_comments, preceding_of_kind, span, string_args, strip_generics,
    strip_ws, text,
};

pub struct RustAdapter;

const COMMENTS: &[&str] = &["line_comment", "block_comment"];
const STRINGS: &[&str] = &["string_literal", "raw_string_literal"];

const SPEC: BodySpec = BodySpec {
    loops: &["for_expression", "while_expression", "loop_expression"],
    conditionals: &["if_expression", "match_arm", "else_clause"],
    branches: &["if_expression"],
    switches: &["match_expression"],
    arms: &["match_arm"],
    binary: &["binary_expression"],
    closures: &["closure_expression", "async_block", "function_item"],
    early_exits: &["return_expression", "try_expression"],
    awaits: &["await_expression"],
    chain: &[
        "call_expression",
        "field_expression",
        "generic_function",
        "try_expression",
    ],
    stop: &[],
    ref_kinds: &["identifier", "scoped_identifier", "field_expression"],
    ref_containers: &["array_expression", "tuple_expression"],
    spawn: &[],
    call: call_info,
    expand: Some(expand_macro),
    idents: &["identifier", "self", "shorthand_field_identifier"],
    binders: &[
        ("let_declaration", "pattern"),
        ("assignment_expression", "left"),
        ("compound_assignment_expr", "left"),
    ],
    mutable: &["mutable_specifier"],
};

/// Re-parses a macro's token tree as Rust expressions so calls inside `join!`, `select!`,
/// `vec!`, `println!`... are seen. Top-level `,` and `=>` become statement separators and the
/// text is padded so line numbers match the original file.
fn expand_macro(node: Node, source: &str) -> Vec<CallSite> {
    let Some(mac) = node.child_by_field_name("macro") else {
        return vec![];
    };
    if text(mac, source) == "macro_rules" {
        return vec![];
    }
    let Some(tokens) = child_of_kind(node, "token_tree") else {
        return vec![];
    };
    let inner = text(tokens, source);
    if inner.len() < 2 {
        return vec![];
    }
    let mut body = String::new();
    let mut depth = 0u32;
    let mut chars = inner[1..inner.len() - 1].chars().peekable();
    let mut in_str = false;
    while let Some(c) = chars.next() {
        if in_str {
            body.push(c);
            if c == '\\' {
                body.extend(chars.next());
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => {
                in_str = true;
                body.push(c);
            }
            '(' | '[' | '{' => {
                depth += 1;
                body.push(c);
            }
            ')' | ']' | '}' => {
                depth = depth.saturating_sub(1);
                body.push(c);
            }
            ',' if depth == 0 => body.push(';'),
            '=' if depth == 0 && chars.peek() == Some(&'>') => {
                chars.next();
                body.push_str("; ");
            }
            _ => body.push(c),
        }
    }
    let padded = format!(
        "{}fn __m() {{ {body} ; }}",
        "\n".repeat(tokens.start_position().row)
    );
    let Ok(tree) = common::parse_tree(&tree_sitter_rust::LANGUAGE.into(), &padded) else {
        return vec![];
    };
    let Some(block) = named_children(tree.root_node())
        .into_iter()
        .find(|n| n.kind() == "function_item")
        .and_then(|f| f.child_by_field_name("body"))
    else {
        return vec![];
    };
    let mut calls = common::analyze_body(block, &padded, &SPEC).calls;
    if matches!(
        common::last_segment(text(mac, source)).as_str(),
        "join" | "try_join" | "select"
    ) {
        for c in &mut calls {
            c.awaited = true;
        }
    }
    calls
}

fn string_arg(node: Node, source: &str) -> Option<String> {
    match node.kind() {
        k if STRINGS.contains(&k) => Some(common::string_value(node, source, &[], &[])),
        "reference_expression" => string_arg(node.child_by_field_name("value")?, source),
        "parenthesized_expression" => named_children(node)
            .into_iter()
            .find_map(|c| string_arg(c, source)),
        "macro_invocation" => {
            let name = last_segment(text(node.child_by_field_name("macro")?, source));
            let tokens = child_of_kind(node, "token_tree")?;
            let literals: Vec<String> = named_children(tokens)
                .into_iter()
                .filter(|c| STRINGS.contains(&c.kind()))
                .map(|c| common::string_value(c, source, &[], &[]))
                .collect();
            match name.as_str() {
                "concat" => Some(literals.concat()),
                "format" | "format_args" => {
                    literals.first().map(|l| common::normalise_placeholders(l))
                }
                _ => None,
            }
        }
        _ => None,
    }
}

fn call_info(node: Node, source: &str) -> Option<CallInfo> {
    match node.kind() {
        "call_expression" => {
            let func = node.child_by_field_name("function")?;
            let target = if func.kind() == "generic_function" {
                func.child_by_field_name("function").unwrap_or(func)
            } else {
                func
            };
            let receiver = (target.kind() == "field_expression")
                .then(|| field_text(target, "value", source))
                .flatten();
            Some(CallInfo::method(
                &strip_generics(text(target, source)),
                receiver,
                string_args(node.child_by_field_name("arguments"), source, string_arg),
            ))
        }
        "macro_invocation" => {
            let mac = node.child_by_field_name("macro")?;
            Some(CallInfo {
                callee: format!("{}!", strip_ws(text(mac, source))),
                receiver: None,
                string_args: string_args(child_of_kind(node, "token_tree"), source, string_arg),
            })
        }
        _ => None,
    }
}

/// `Enum::Variant` for a variant expression carrying data (`A::B { .. }`, `A::B(..)`), or
/// a pattern destructuring one.
fn variant_path(node: Node, source: &str) -> Option<String> {
    let path = match node.kind() {
        "struct_expression" => node.child_by_field_name("name")?,
        "call_expression" => node.child_by_field_name("function")?,
        "struct_pattern" => node.child_by_field_name("type")?,
        "tuple_struct_pattern" => node.child_by_field_name("type")?,
        _ => return None,
    };
    if !matches!(path.kind(), "scoped_identifier" | "scoped_type_identifier") {
        return None;
    }
    let full = strip_generics(text(path, source));
    let segs: Vec<&str> = full.split("::").map(str::trim).collect();
    let n = segs.len();
    if n < 2 {
        return None;
    }
    let (ty, variant) = (segs[n - 2], segs[n - 1]);
    let typed = |s: &str| s.starts_with(|c: char| c.is_ascii_uppercase());
    if !typed(ty) || !typed(variant) || matches!(ty, "Some" | "Ok" | "Err" | "Box" | "Self") {
        return None;
    }
    Some(format!("{ty}::{variant}"))
}

/// Message passing seen through enum values: a variant with data handed to a method call
/// (`tx.send(frame)`, `bus.deliver(Msg::X { .. })`) is a `<message>` site; a match arm or
/// `if let` destructuring one is a `<match>` site. Both carry the variant as their first
/// string argument so packs can turn them into publish/subscribe effects on a topic.
fn message_sites(body: Node, source: &str) -> Vec<CallSite> {
    let mut bound: Vec<(String, String)> = Vec::new();
    let mut out = Vec::new();
    let mut pending = vec![body];
    let site = |callee: &str, receiver: Option<String>, variant: String, node: Node| CallSite {
        callee: callee.to_owned(),
        name: callee.to_owned(),
        receiver,
        string_args: vec![variant],
        line: common::line(node),
        loop_depth: 0,
        awaited: false,
        conditional: false,
        in_closure: false,
        binds: vec![],
        uses: vec![],
        mutates: vec![],
    };
    while let Some(node) = pending.pop() {
        match node.kind() {
            "let_declaration" => {
                if let (Some(pat), Some(value)) = (
                    node.child_by_field_name("pattern"),
                    node.child_by_field_name("value"),
                ) && pat.kind() == "identifier"
                    && let Some(variant) = variant_path(value, source)
                {
                    bound.push((text(pat, source).to_owned(), variant));
                }
            }
            "call_expression" => {
                let func = node.child_by_field_name("function");
                let receiver = func
                    .filter(|f| f.kind() == "field_expression")
                    .and_then(|f| field_text(f, "value", source));
                if receiver.is_some()
                    && let Some(args) = node.child_by_field_name("arguments")
                {
                    for arg in named_children(args) {
                        let inner = if arg.kind() == "reference_expression" {
                            arg.child_by_field_name("value").unwrap_or(arg)
                        } else {
                            arg
                        };
                        let variant = variant_path(inner, source).or_else(|| {
                            (inner.kind() == "identifier").then(|| {
                                let name = text(inner, source);
                                bound
                                    .iter()
                                    .rev()
                                    .find(|(n, _)| n == name)
                                    .map(|(_, v)| v.clone())
                            })?
                        });
                        if let Some(variant) = variant {
                            out.push(site("<message>", receiver.clone(), variant, node));
                        }
                    }
                }
            }
            "match_arm" | "let_condition" => {
                let pat = node.child_by_field_name("pattern").map(|p| {
                    if p.kind() == "match_pattern" {
                        p.named_child(0).unwrap_or(p)
                    } else {
                        p
                    }
                });
                if let Some(variant) = pat.and_then(|p| variant_path(p, source)) {
                    out.push(site("<match>", None, variant, node));
                }
            }
            _ => {}
        }
        pending.extend(named_children(node).into_iter().rev());
    }
    out
}

const CHAIN: ChainSpec = ChainSpec {
    ifs: &["if_expression"],
    elifs: &[],
    elses: &["else_clause"],
};

const LITERALS: &[&str] = &["string_literal", "integer_literal", "char_literal"];

fn literal(node: Node, source: &str) -> Option<String> {
    LITERALS
        .contains(&node.kind())
        .then(|| common::string_value(node, source, &[], &[]))
}

/// Values a pattern stands for; `None` when the pattern is not a plain value or wildcard
/// (tuples, ranges, `Some(..)`), which disqualifies the whole site.
fn pattern_values(node: Node, source: &str, out: &mut Vec<Handled>) -> Option<bool> {
    match node.kind() {
        "match_pattern" | "or_pattern" => {
            let mut wildcard = false;
            let guard = node.child_by_field_name("condition");
            for c in all_children(node) {
                if (c.is_named() || c.kind() == "_") && guard != Some(c) {
                    wildcard |= pattern_values(c, source, out)?;
                }
            }
            Some(wildcard)
        }
        "_" => Some(true),
        "identifier" => {
            let t = text(node, source);
            if t.starts_with(|c: char| c.is_ascii_uppercase()) {
                out.push(Handled::literal(t.to_owned()));
                Some(false)
            } else {
                Some(true)
            }
        }
        "scoped_identifier" => {
            out.push(Handled::member(&strip_generics(text(node, source)), "::")?);
            Some(false)
        }
        "tuple_struct_pattern" | "struct_pattern" => {
            out.push(Handled::member(&variant_path(node, source)?, "::")?);
            Some(false)
        }
        k if LITERALS.contains(&k) => {
            out.push(Handled::literal(literal(node, source)?));
            Some(false)
        }
        _ => None,
    }
}

/// `x == S::A`, `a == 1 || a == 2`, `matches!(x, A | B)`, `let S::A = x`.
fn compare(node: Node, source: &str) -> Option<Comparison> {
    match node.kind() {
        "parenthesized_expression" => compare(named_children(node).into_iter().next()?, source),
        "binary_expression" => {
            let op = text(node.child_by_field_name("operator")?, source);
            let left = node.child_by_field_name("left")?;
            let right = node.child_by_field_name("right")?;
            match op {
                "||" => Comparison::merge(compare(left, source), compare(right, source)),
                "==" => {
                    let value = |n: Node| {
                        literal(n, source).map(Handled::literal).or_else(|| {
                            (n.kind() == "scoped_identifier")
                                .then(|| Handled::member(text(n, source), "::"))
                                .flatten()
                        })
                    };
                    match (value(right), value(left)) {
                        (Some(v), _) => Comparison::new(text(left, source), vec![v]),
                        (None, Some(v)) => Comparison::new(text(right, source), vec![v]),
                        _ => None,
                    }
                }
                _ => None,
            }
        }
        "let_condition" => {
            let mut values = Vec::new();
            let wildcard =
                pattern_values(node.child_by_field_name("pattern")?, source, &mut values)?;
            if wildcard {
                return None;
            }
            Comparison::new(text(node.child_by_field_name("value")?, source), values)
        }
        "macro_invocation" => {
            if last_segment(text(node.child_by_field_name("macro")?, source)) != "matches" {
                return None;
            }
            let tokens = text(child_of_kind(node, "token_tree")?, source);
            let inner = tokens.get(1..tokens.len() - 1)?;
            let (subject, pats) = inner.split_once(',')?;
            let padded = format!(
                "fn __m(x: i32) {{ match x {{ {} => 1, _ => 2 }} }}",
                pats.trim()
            );
            let tree = common::parse_tree(&tree_sitter_rust::LANGUAGE.into(), &padded).ok()?;
            let mut arm = None;
            let mut pending = vec![tree.root_node()];
            while let Some(n) = pending.pop() {
                if n.kind() == "match_arm" {
                    arm = Some(n);
                    break;
                }
                pending.extend(named_children(n).into_iter().rev());
            }
            let mut values = Vec::new();
            let wildcard =
                pattern_values(arm?.child_by_field_name("pattern")?, &padded, &mut values)?;
            if wildcard {
                return None;
            }
            Comparison::new(subject, values)
        }
        _ => None,
    }
}

fn match_site(node: Node, source: &str, sites: &mut Sites) {
    let Some(subject) = node.child_by_field_name("value") else {
        return;
    };
    let Some(body) = node.child_by_field_name("body") else {
        return;
    };
    let mut cmp = Comparison {
        subject: strip_ws(text(subject, source)),
        ..Comparison::default()
    };
    let mut default = DefaultKind::None;
    for arm in named_children(body) {
        if arm.kind() != "match_arm" {
            continue;
        }
        let Some(pat) = arm.child_by_field_name("pattern") else {
            continue;
        };
        let mut values = Vec::new();
        let Some(wildcard) = pattern_values(pat, source, &mut values) else {
            return;
        };
        cmp.extend(values);
        if wildcard && pat.child_by_field_name("condition").is_none() {
            default = arm
                .child_by_field_name("value")
                .map_or(DefaultKind::Other, |v| {
                    common::classify_default(text(v, source))
                });
        }
    }
    sites.push(common::line(node), cmp, default);
}

fn discriminations(body: Node, source: &str) -> Vec<hoshizora_model::Discrimination> {
    let mut sites = Sites::default();
    let mut pending = vec![body];
    while let Some(node) = pending.pop() {
        match node.kind() {
            "match_expression" => match_site(node, source, &mut sites),
            "if_expression" => sites.if_chain(node, source, &CHAIN, &compare, &|b, s| {
                text(b, s).to_owned()
            }),
            _ => {}
        }
        pending.extend(named_children(node).into_iter().rev());
    }
    sites.out
}

fn enum_domain(node: Node, source: &str) -> Option<Domain> {
    let body = node.child_by_field_name("body")?;
    let values: Vec<DomainValue> = named_children(body)
        .into_iter()
        .filter(|c| c.kind() == "enum_variant")
        .filter_map(|c| {
            Some(DomainValue {
                name: field_text(c, "name", source)?,
                literal: None,
            })
        })
        .collect();
    (values.len() >= 2).then_some(Domain {
        name: field_text(node, "name", source)?,
        values,
        span: span(node),
    })
}

fn doc(node: Node, source: &str) -> Option<String> {
    let lines: Vec<&str> = preceding_comments(node, COMMENTS, &["attribute_item"])
        .into_iter()
        .filter(|c| {
            c.child_by_field_name("doc").is_some() && c.child_by_field_name("inner").is_none()
        })
        .map(|c| text(c, source))
        .collect();
    clean_comment_lines(&lines, &["///", "/**", "*"])
}

fn attributes(node: Node, source: &str) -> Vec<String> {
    preceding_of_kind(node, "attribute_item", COMMENTS)
        .into_iter()
        .filter_map(|a| child_of_kind(a, "attribute"))
        .map(|a| collapse_ws(text(a, source)))
        .collect()
}

fn struct_fields(node: Node, source: &str) -> Vec<Param> {
    let Some(body) = node.child_by_field_name("body") else {
        return vec![];
    };
    named_children(body)
        .into_iter()
        .filter(|c| c.kind() == "field_declaration")
        .filter_map(|c| {
            Some(Param {
                name: field_text(c, "name", source)?,
                ty: field_text(c, "type", source).map(|t| strip_ws(&t)),
                default: None,
            })
        })
        .collect()
}

fn params(node: Node, source: &str) -> Vec<Param> {
    let Some(list) = node.child_by_field_name("parameters") else {
        return vec![];
    };
    named_children(list)
        .into_iter()
        .filter_map(|p| match p.kind() {
            "parameter" => Some(Param {
                name: field_text(p, "pattern", source)?,
                ty: field_text(p, "type", source),
                default: None,
            }),
            "self_parameter" => Some(Param {
                name: "self".into(),
                ty: Some(collapse_ws(text(p, source))),
                default: None,
            }),
            "variadic_parameter" => Some(Param {
                name: "...".into(),
                ty: None,
                default: None,
            }),
            _ => None,
        })
        .collect()
}

fn is_pub(node: Node) -> bool {
    has_child_kind(node, "visibility_modifier")
}

fn is_async(node: Node, source: &str) -> bool {
    child_of_kind(node, "function_modifiers").is_some_and(|m| text(m, source).contains("async"))
}

struct Ctx<'a> {
    source: &'a str,
    symbols: Vec<Symbol>,
    imports: Vec<Import>,
    domains: Vec<Domain>,
    trait_impls: Vec<(String, String)>,
}

impl Ctx<'_> {
    fn function(&mut self, node: Node, parent: Option<&str>, public: bool) {
        let name = field_text(node, "name", self.source).unwrap_or_default();
        let local_name = parent.map_or_else(|| name.clone(), |p| format!("{p}::{name}"));
        self.symbols.push(Symbol {
            kind: if parent.is_some() {
                SymbolKind::Method
            } else {
                SymbolKind::Function
            },
            local_name,
            parent: parent.map(str::to_string),
            signature: header(node, self.source, "body"),
            params: params(node, self.source),
            returns: field_text(node, "return_type", self.source),
            doc: doc(node, self.source),
            span: span(node),
            is_async: is_async(node, self.source),
            visibility: if public || is_pub(node) {
                Visibility::Public
            } else {
                Visibility::Private
            },
            attributes: attributes(node, self.source),
            implements: vec![],
            body: node.child_by_field_name("body").map(|b| {
                let mut body = common::analyze_body(b, self.source, &SPEC);
                body.calls.extend(message_sites(b, self.source));
                body.calls.sort_by_key(|c| c.line);
                body.discriminations = discriminations(b, self.source);
                body
            }),
            name,
        });
    }

    fn plain(&mut self, node: Node, kind: SymbolKind) {
        let name = field_text(node, "name", self.source).unwrap_or_default();
        if node.kind() == "enum_item" {
            self.domains.extend(enum_domain(node, self.source));
        }
        let mut attrs = attributes(node, self.source);
        if let Some(body) = node.child_by_field_name("body") {
            attrs.extend(
                named_children(body)
                    .into_iter()
                    .filter(|c| c.kind() == "attribute_item")
                    .filter_map(|a| child_of_kind(a, "attribute"))
                    .map(|a| collapse_ws(text(a, self.source))),
            );
        }
        let fields = if node.kind() == "struct_item" {
            struct_fields(node, self.source)
        } else {
            vec![]
        };
        self.symbols.push(Symbol {
            kind,
            local_name: name.clone(),
            parent: None,
            signature: header(node, self.source, "body"),
            params: fields,
            returns: field_text(node, "type", self.source),
            doc: doc(node, self.source),
            span: span(node),
            is_async: false,
            visibility: if is_pub(node) {
                Visibility::Public
            } else {
                Visibility::Private
            },
            attributes: attrs,
            implements: vec![],
            body: None,
            name,
        });
    }

    fn item(&mut self, node: Node) {
        match node.kind() {
            "function_item" => self.function(node, None, false),
            "struct_item" | "enum_item" | "union_item" | "type_item" => {
                self.plain(node, SymbolKind::Type);
            }
            "const_item" | "static_item" => self.plain(node, SymbolKind::Constant),
            "trait_item" => {
                self.plain(node, SymbolKind::Interface);
                let name = field_text(node, "name", self.source).unwrap_or_default();
                let public = is_pub(node);
                if let Some(body) = node.child_by_field_name("body") {
                    for m in named_children(body) {
                        if matches!(m.kind(), "function_item" | "function_signature_item") {
                            self.function(m, Some(&name), public);
                        }
                    }
                }
            }
            "impl_item" => {
                let ty = node
                    .child_by_field_name("type")
                    .map(|t| strip_generics(text(t, self.source)))
                    .unwrap_or_default();
                let trait_ = node
                    .child_by_field_name("trait")
                    .map(|t| strip_ws(text(t, self.source)));
                if let Some(t) = &trait_ {
                    self.trait_impls.push((ty.clone(), t.clone()));
                }
                if let Some(body) = node.child_by_field_name("body") {
                    for m in named_children(body) {
                        if m.kind() == "function_item" {
                            self.function(m, Some(&ty), trait_.is_some());
                        }
                    }
                }
            }
            "mod_item" => {
                if let Some(body) = node.child_by_field_name("body") {
                    for c in named_children(body) {
                        self.item(c);
                    }
                }
            }
            "use_declaration" => self.import(node),
            _ => {}
        }
    }

    fn import(&mut self, node: Node) {
        let Some(arg) = node.child_by_field_name("argument") else {
            return;
        };
        let line = common::line(node);
        let src = self.source;
        match arg.kind() {
            "use_as_clause" => self.imports.push(Import {
                path: strip_ws(&field_text(arg, "path", src).unwrap_or_default()),
                alias: field_text(arg, "alias", src),
                names: vec![],
                line,
            }),
            "scoped_use_list" => {
                let names = arg
                    .child_by_field_name("list")
                    .map(|l| {
                        named_children(l)
                            .iter()
                            .map(|n| collapse_ws(text(*n, src)))
                            .collect()
                    })
                    .unwrap_or_default();
                self.imports.push(Import {
                    path: strip_ws(&field_text(arg, "path", src).unwrap_or_default()),
                    alias: None,
                    names,
                    line,
                });
            }
            "use_list" => self.imports.push(Import {
                path: String::new(),
                alias: None,
                names: named_children(arg)
                    .iter()
                    .map(|n| collapse_ws(text(*n, src)))
                    .collect(),
                line,
            }),
            _ => self.imports.push(Import {
                path: strip_ws(text(arg, src)),
                alias: None,
                names: vec![],
                line,
            }),
        }
    }
}

impl Adapter for RustAdapter {
    fn language(&self) -> Language {
        Language::Rust
    }

    fn extensions(&self) -> &[&str] {
        &["rs"]
    }

    fn parse(&self, rel_path: &str, source: &str, module: &str) -> anyhow::Result<FileFacts> {
        let tree = common::parse_tree(&tree_sitter_rust::LANGUAGE.into(), source)?;
        let mut ctx = Ctx {
            source,
            symbols: vec![],
            imports: vec![],
            domains: vec![],
            trait_impls: vec![],
        };
        for item in all_children(tree.root_node()) {
            ctx.item(item);
        }
        for (ty, tr) in ctx.trait_impls {
            if let Some(s) = ctx
                .symbols
                .iter_mut()
                .find(|s| s.kind == SymbolKind::Type && s.name == ty)
            {
                s.implements.push(tr);
            }
        }
        Ok(FileFacts {
            path: rel_path.to_string(),
            language: Language::Rust,
            module: module.to_string(),
            lines: common::line_count(source),
            imports: ctx.imports,
            symbols: ctx.symbols,
            domains: ctx.domains,
        })
    }
}
