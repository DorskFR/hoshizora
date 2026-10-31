use std::collections::HashSet;

use hoshizora_model::{Body, FileFacts, Import, Language, Param, Symbol, SymbolKind, Visibility};
use tree_sitter::Node;

use crate::Adapter;
use crate::common::{
    self, BodySpec, CallInfo, all_children, child_of_kind, clean_comment_lines, collapse_ws,
    field_text, has_child_kind, named_children, preceding_comments, span, string_args, strip_ws,
    text,
};

#[derive(Clone, Copy)]
pub enum Flavor {
    TypeScript,
    Tsx,
    JavaScript,
}

pub struct TsAdapter(pub Flavor);

const STRINGS: &[&str] = &["string", "template_string"];
const INTERP: &[&str] = &["template_substitution"];
const FUNCS: &[&str] = &["arrow_function", "function_expression", "function"];

const SPEC: BodySpec = BodySpec {
    loops: &[
        "for_statement",
        "for_in_statement",
        "while_statement",
        "do_statement",
    ],
    conditionals: &[
        "if_statement",
        "else_clause",
        "switch_case",
        "switch_default",
        "ternary_expression",
        "catch_clause",
    ],
    branches: &["if_statement", "ternary_expression", "catch_clause"],
    switches: &["switch_body"],
    arms: &["switch_case", "switch_default"],
    binary: &["binary_expression"],
    closures: &[
        "arrow_function",
        "function_expression",
        "function",
        "function_declaration",
    ],
    early_exits: &["return_statement", "throw_statement"],
    awaits: &["await_expression"],
    chain: &[
        "call_expression",
        "member_expression",
        "as_expression",
        "non_null_expression",
        "parenthesized_expression",
    ],
    stop: &["class_declaration"],
    ref_kinds: &["identifier", "member_expression"],
    ref_containers: &["array", "pair"],
    spawn: &[],
    call: call_info,
    expand: None,
};

fn string_arg(node: Node, source: &str) -> Option<String> {
    match node.kind() {
        k if STRINGS.contains(&k) => Some(common::string_value(node, source, INTERP, &[])),
        "parenthesized_expression" => named_children(node)
            .into_iter()
            .find_map(|c| string_arg(c, source)),
        "binary_expression" => common::concat_value(node, source, "+", string_arg),
        _ => None,
    }
}

fn call_info(node: Node, source: &str) -> Option<CallInfo> {
    let func = match node.kind() {
        "call_expression" => node.child_by_field_name("function")?,
        "new_expression" => node.child_by_field_name("constructor")?,
        _ => return None,
    };
    if FUNCS.contains(&func.kind()) {
        return None;
    }
    let receiver = (func.kind() == "member_expression")
        .then(|| field_text(func, "object", source))
        .flatten();
    Some(CallInfo::method(
        text(func, source),
        receiver,
        string_args(node.child_by_field_name("arguments"), source, string_arg),
    ))
}

fn doc(node: Node, source: &str) -> Option<String> {
    let lines: Vec<&str> = preceding_comments(node, &["comment"], &["decorator"])
        .into_iter()
        .map(|c| text(c, source))
        .filter(|t| t.starts_with("/**"))
        .collect();
    clean_comment_lines(&lines, &["/**", "*"])
}

fn decorators(node: Node, source: &str) -> Vec<String> {
    let mut out: Vec<String> = common::preceding_of_kind(node, "decorator", &["comment"])
        .into_iter()
        .map(|d| text(d, source))
        .map(|t| collapse_ws(t.trim_start_matches('@')))
        .collect();
    out.extend(
        node.children_by_field_name("decorator", &mut node.walk())
            .map(|d| collapse_ws(text(d, source).trim_start_matches('@'))),
    );
    out
}

fn type_text(node: Node, field: &str, source: &str) -> Option<String> {
    node.child_by_field_name(field)
        .map(|t| collapse_ws(text(t, source).trim_start_matches(':').trim()))
}

fn params(func: Node, source: &str) -> Vec<Param> {
    if let Some(p) = func.child_by_field_name("parameter") {
        return vec![Param {
            name: text(p, source).into(),
            ty: None,
            default: None,
        }];
    }
    let Some(list) = func.child_by_field_name("parameters") else {
        return vec![];
    };
    named_children(list)
        .into_iter()
        .filter_map(|p| match p.kind() {
            "required_parameter" | "optional_parameter" => Some(Param {
                name: field_text(p, "pattern", source)?,
                ty: type_text(p, "type", source),
                default: field_text(p, "value", source),
            }),
            "rest_parameter" => Some(Param {
                name: format!("...{}", field_text(p, "pattern", source)?),
                ty: type_text(p, "type", source),
                default: None,
            }),
            "identifier" | "rest_pattern" | "object_pattern" | "array_pattern"
            | "assignment_pattern" => {
                let (name, default) = if p.kind() == "assignment_pattern" {
                    (
                        field_text(p, "left", source)?,
                        field_text(p, "right", source),
                    )
                } else {
                    (collapse_ws(text(p, source)), None)
                };
                Some(Param {
                    name,
                    ty: None,
                    default,
                })
            }
            _ => None,
        })
        .collect()
}

fn has_method(object: Node) -> bool {
    named_children(object).iter().any(|p| {
        p.kind() == "method_definition"
            || (p.kind() == "pair"
                && p.child_by_field_name("value")
                    .is_some_and(|v| FUNCS.contains(&v.kind())))
    })
}

/// A class field as a `Param`: its declared type, else the callee or constructor that
/// initialises it (`useX()` → `useX()`, `new Store()` → `Store`).
fn field(member: Node, source: &str) -> Option<Param> {
    if member.kind() != "public_field_definition" {
        return None;
    }
    let value = member.child_by_field_name("value");
    if value.is_some_and(|v| FUNCS.contains(&v.kind())) {
        return None;
    }
    let name = field_text(member, "name", source)?;
    let ty = type_text(member, "type", source).or_else(|| {
        let v = value?;
        match v.kind() {
            "call_expression" => Some(format!(
                "{}()",
                strip_ws(&field_text(v, "function", source)?)
            )),
            "new_expression" => field_text(v, "constructor", source),
            _ => None,
        }
    });
    Some(Param {
        name,
        ty,
        default: None,
    })
}

/// Names bound to functions anywhere in the file: declarations, methods, object-literal
/// methods, function-valued bindings (including `wrap(fn)` of a known function) and imports.
fn known_functions(root: Node, source: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    let mut wrapped: Vec<(String, Vec<String>)> = Vec::new();
    let mut pending = vec![root];
    while let Some(node) = pending.pop() {
        match node.kind() {
            "function_declaration" | "generator_function_declaration" | "method_definition" => {
                out.extend(field_text(node, "name", source));
            }
            "variable_declarator" | "public_field_definition" | "pair" => {
                let value = node.child_by_field_name("value");
                let args = value
                    .filter(|v| v.kind() == "call_expression")
                    .and_then(|v| v.child_by_field_name("arguments"))
                    .map(named_children)
                    .unwrap_or_default();
                let callable = value.is_some_and(|v| FUNCS.contains(&v.kind()))
                    || args.iter().any(|x| FUNCS.contains(&x.kind()));
                let idents: Vec<String> = args
                    .iter()
                    .filter(|x| x.kind() == "identifier")
                    .map(|x| text(*x, source).to_owned())
                    .collect();
                if !callable
                    && !idents.is_empty()
                    && node.kind() == "variable_declarator"
                    && let Some(name) = field_text(node, "name", source)
                {
                    wrapped.push((name, idents));
                }
                if callable {
                    let key = if node.kind() == "pair" { "key" } else { "name" };
                    out.extend(
                        field_text(node, key, source)
                            .map(|k| k.trim_matches(['"', '\'']).to_owned()),
                    );
                }
            }
            "import_specifier" => {
                out.extend(
                    field_text(node, "alias", source).or_else(|| field_text(node, "name", source)),
                );
            }
            "import_clause" => {
                for c in named_children(node) {
                    match c.kind() {
                        "identifier" => out.insert(text(c, source).to_owned()),
                        "namespace_import" => named_children(c)
                            .first()
                            .is_some_and(|n| out.insert(text(*n, source).to_owned())),
                        _ => false,
                    };
                }
            }
            _ => {}
        }
        pending.extend(named_children(node));
    }
    for (name, idents) in wrapped {
        if idents.iter().any(|i| out.contains(i)) {
            out.insert(name);
        }
    }
    out
}

fn exported(node: Node) -> bool {
    node.parent()
        .is_some_and(|p| p.kind() == "export_statement")
}

struct Ctx<'a> {
    source: &'a str,
    symbols: Vec<Symbol>,
    imports: Vec<Import>,
    /// `(outer statement, declarator, call initializer, name, exported)` awaiting a decision
    /// on whether the bound value is callable.
    candidates: Vec<(Node<'a>, Node<'a>, Node<'a>, String, bool)>,
    /// Declarators emitted as symbols, excluded from `<module>`.
    bound: Vec<usize>,
    /// Names that denote functions in this file (declared, imported, object-literal methods):
    /// only these count as callbacks when passed bare as arguments.
    known: HashSet<String>,
}

impl<'a> Ctx<'a> {
    fn function(
        &mut self,
        decl: Node,
        func: Node,
        name: String,
        parent: Option<&str>,
        public: bool,
        attributes: Vec<String>,
    ) {
        let body = func.child_by_field_name("body");
        let local_name = parent.map_or_else(|| name.clone(), |p| format!("{p}.{name}"));
        let returned = body
            .filter(|b| b.kind() == "statement_block")
            .map(|b| self.returned_methods(b, &local_name, public))
            .unwrap_or_default();
        self.symbols.push(Symbol {
            kind: if parent.is_some() {
                SymbolKind::Method
            } else {
                SymbolKind::Function
            },
            local_name,
            parent: parent.map(str::to_string),
            signature: {
                let end = body.map_or(decl.end_byte(), |b| b.start_byte());
                let sig = collapse_ws(&self.source[decl.start_byte()..end]);
                let sig = sig.trim_end_matches(['{', ' ']);
                let sig = sig.strip_suffix("=>").unwrap_or(sig).trim_end();
                sig.strip_suffix('=').unwrap_or(sig).trim_end().to_string()
            },
            params: params(func, self.source),
            returns: type_text(func, "return_type", self.source),
            doc: doc(decl, self.source),
            span: span(decl),
            is_async: has_child_kind(func, "async"),
            visibility: if public {
                Visibility::Public
            } else {
                Visibility::Private
            },
            attributes,
            implements: vec![],
            body: body.map(|b| {
                let mut body = if b.kind() == "statement_block" {
                    self.body(&named_children(b))
                } else {
                    self.body(&named_children(func))
                };
                body.calls.retain(|c| {
                    !returned
                        .iter()
                        .any(|(start, end)| *start <= c.line && c.line <= *end)
                });
                body
            }),
            name,
        });
    }

    fn body(&self, nodes: &[Node]) -> Body {
        let known = &self.known;
        common::analyze_nodes_with(nodes, self.source, &SPEC, &|t| {
            t.contains('.') || known.contains(t)
        })
    }

    /// Methods of an object literal: `{ spawn: (b) => ..., run() {...} }`.
    fn object_methods(&mut self, object: Node, parent: &str, public: bool) -> Vec<(u32, u32)> {
        let src = self.source;
        let mut spans = Vec::new();
        for prop in named_children(object) {
            match prop.kind() {
                "pair" => {
                    if let Some(v) = prop
                        .child_by_field_name("value")
                        .filter(|v| FUNCS.contains(&v.kind()))
                        && let Some(key) = field_text(prop, "key", src)
                    {
                        let key = key.trim_matches(['"', '\'']).to_owned();
                        let sp = span(prop);
                        spans.push((sp.start_line, sp.end_line));
                        self.function(prop, v, key, Some(parent), public, vec![]);
                    }
                }
                "method_definition" => {
                    let key = field_text(prop, "name", src).unwrap_or_default();
                    let sp = span(prop);
                    spans.push((sp.start_line, sp.end_line));
                    self.function(prop, prop, key, Some(parent), public, vec![]);
                }
                _ => {}
            }
        }
        spans
    }

    /// Object literals returned by a function become its methods (`useX()` factories).
    fn returned_methods(&mut self, block: Node, parent: &str, public: bool) -> Vec<(u32, u32)> {
        let mut spans = Vec::new();
        for stmt in named_children(block) {
            if stmt.kind() == "return_statement"
                && let Some(obj) = named_children(stmt)
                    .into_iter()
                    .find(|n| n.kind() == "object")
            {
                spans.extend(self.object_methods(obj, parent, public));
            }
        }
        spans
    }

    fn plain(
        &mut self,
        node: Node,
        inner: Node,
        kind: SymbolKind,
        name: String,
        public: bool,
        implements: Vec<String>,
    ) {
        let end = inner
            .child_by_field_name("body")
            .map_or(node.end_byte(), |b| b.start_byte());
        let signature = collapse_ws(&self.source[node.start_byte()..end]);
        self.symbols.push(Symbol {
            kind,
            local_name: name.clone(),
            parent: None,
            signature: signature.trim_end_matches([' ', '{', ';']).to_string(),
            params: vec![],
            returns: None,
            doc: doc(node, self.source),
            span: span(node),
            is_async: false,
            visibility: if public {
                Visibility::Public
            } else {
                Visibility::Private
            },
            attributes: decorators(node, self.source),
            implements,
            body: None,
            name,
        });
    }

    fn statement(&mut self, node: Node<'a>) {
        let public = exported(node);
        let outer = if public {
            node.parent().unwrap_or(node)
        } else {
            node
        };
        let src = self.source;
        match node.kind() {
            "export_statement" => {
                if let Some(decl) = node.child_by_field_name("declaration") {
                    self.statement(decl);
                } else if let Some(v) = node.child_by_field_name("value")
                    && FUNCS.contains(&v.kind())
                {
                    let name = field_text(v, "name", src).unwrap_or_else(|| "default".into());
                    self.function(node, v, name, None, true, vec![]);
                }
            }
            "function_declaration" | "generator_function_declaration" => {
                let name = field_text(node, "name", src).unwrap_or_default();
                self.function(outer, node, name, None, public, vec![]);
            }
            "lexical_declaration" | "variable_declaration" => {
                for d in named_children(node)
                    .into_iter()
                    .filter(|d| d.kind() == "variable_declarator")
                {
                    let Some(name) = field_text(d, "name", src) else {
                        continue;
                    };
                    match d.child_by_field_name("value") {
                        Some(v) if FUNCS.contains(&v.kind()) => {
                            self.bound.push(d.id());
                            self.function(outer, v, name, None, public, vec![]);
                        }
                        Some(v) if v.kind() == "call_expression" => {
                            self.candidates.push((outer, d, v, name, public));
                        }
                        Some(v) if v.kind() == "object" && has_method(v) => {
                            self.bound.push(d.id());
                            self.object_methods(v, &name, public);
                        }
                        _ if name
                            .chars()
                            .all(|c| c.is_ascii_uppercase() || c == '_' || c.is_ascii_digit())
                            && name.chars().any(|c| c.is_ascii_uppercase()) =>
                        {
                            self.plain(outer, d, SymbolKind::Constant, name, public, vec![]);
                        }
                        _ => {}
                    }
                }
            }
            "class_declaration" | "abstract_class_declaration" | "class" => {
                let name = field_text(node, "name", src).unwrap_or_default();
                let implements = child_of_kind(node, "class_heritage")
                    .map(|h| {
                        named_children(h)
                            .into_iter()
                            .flat_map(|c| {
                                let inner = named_children(c);
                                if inner.is_empty() { vec![c] } else { inner }
                            })
                            .map(|n| strip_ws(text(n, src)))
                            .collect()
                    })
                    .unwrap_or_default();
                self.plain(
                    outer,
                    node,
                    SymbolKind::Type,
                    name.clone(),
                    public,
                    implements,
                );
                if let Some(body) = node.child_by_field_name("body") {
                    let fields: Vec<Param> = named_children(body)
                        .into_iter()
                        .filter_map(|m| field(m, src))
                        .collect();
                    if let Some(class) = self.symbols.last_mut() {
                        class.params = fields;
                    }
                    for m in named_children(body) {
                        self.member(m, &name);
                    }
                }
            }
            "interface_declaration" | "type_alias_declaration" | "enum_declaration" => {
                let kind = if node.kind() == "interface_declaration" {
                    SymbolKind::Interface
                } else {
                    SymbolKind::Type
                };
                let implements = child_of_kind(node, "extends_type_clause")
                    .map(|e| {
                        named_children(e)
                            .into_iter()
                            .map(|n| strip_ws(text(n, src)))
                            .collect()
                    })
                    .unwrap_or_default();
                let name = field_text(node, "name", src).unwrap_or_default();
                self.plain(outer, node, kind, name, public, implements);
            }
            "import_statement" => self.import(node),
            _ => {}
        }
    }

    fn member(&mut self, node: Node, class: &str) {
        let src = self.source;
        let private = has_child_kind(node, "accessibility_modifier")
            && child_of_kind(node, "accessibility_modifier")
                .is_some_and(|m| text(m, src) != "public")
            || node
                .child_by_field_name("name")
                .is_some_and(|n| n.kind() == "private_property_identifier");
        match node.kind() {
            "method_definition" => {
                let name = field_text(node, "name", src).unwrap_or_default();
                let attrs = decorators(node, src);
                self.function(node, node, name, Some(class), !private, attrs);
            }
            "public_field_definition" => {
                if let Some(v) = node
                    .child_by_field_name("value")
                    .filter(|v| FUNCS.contains(&v.kind()))
                {
                    let name = field_text(node, "name", src).unwrap_or_default();
                    let attrs = decorators(node, src);
                    self.function(node, v, name, Some(class), !private, attrs);
                }
            }
            _ => {}
        }
    }

    fn import(&mut self, node: Node) {
        let src = self.source;
        let path = node
            .child_by_field_name("source")
            .map(|s| common::string_value(s, src, &[], &[]))
            .unwrap_or_default();
        let mut names = vec![];
        let mut alias = None;
        if let Some(clause) = child_of_kind(node, "import_clause") {
            for c in named_children(clause) {
                match c.kind() {
                    "identifier" => alias = Some(text(c, src).to_string()),
                    "namespace_import" => {
                        alias = named_children(c).first().map(|n| text(*n, src).to_string())
                    }
                    "named_imports" => {
                        names.extend(
                            named_children(c)
                                .into_iter()
                                .filter_map(|s| field_text(s, "name", src)),
                        );
                    }
                    _ => {}
                }
            }
        }
        self.imports.push(Import {
            path,
            alias,
            names,
            line: common::line(node),
        });
    }
}

impl<'a> Ctx<'a> {
    /// Emits bindings whose initializer takes a function literal or a known function by name.
    fn callable_bindings(&mut self) {
        let src = self.source;
        let fn_names: Vec<String> = self
            .symbols
            .iter()
            .filter(|s| s.kind == SymbolKind::Function)
            .map(|s| s.name.clone())
            .collect();
        for (outer, decl, call, name, public) in std::mem::take(&mut self.candidates) {
            let args = call
                .child_by_field_name("arguments")
                .map(named_children)
                .unwrap_or_default();
            let literal = args.iter().copied().find(|a| FUNCS.contains(&a.kind()));
            let by_name = args
                .iter()
                .any(|a| a.kind() == "identifier" && fn_names.iter().any(|n| n == text(*a, src)));
            if literal.is_none() && !by_name {
                continue;
            }
            self.bound.push(decl.id());
            let end = literal
                .and_then(|f| f.child_by_field_name("body"))
                .map_or(decl.end_byte(), |b| b.start_byte());
            let signature = collapse_ws(&src[outer.start_byte()..end]);
            let signature = signature.trim_end_matches([' ', '{', '=', '>']).to_string();
            let body = self.body(&[call]);
            self.symbols.push(Symbol {
                kind: SymbolKind::Function,
                local_name: name.clone(),
                parent: None,
                signature,
                params: literal.map(|f| params(f, src)).unwrap_or_default(),
                returns: literal.and_then(|f| type_text(f, "return_type", src)),
                doc: doc(outer, src),
                span: span(outer),
                is_async: literal.is_some_and(|f| has_child_kind(f, "async")),
                visibility: if public {
                    Visibility::Public
                } else {
                    Visibility::Private
                },
                attributes: vec![],
                implements: vec![],
                body: Some(body),
                name,
            });
        }
    }

    /// Top-level statements that run at load, minus declarations emitted as symbols.
    fn module_nodes(&self, root: Node<'a>) -> Vec<Node<'a>> {
        let mut out = vec![];
        for stmt in named_children(root) {
            let target = if stmt.kind() == "export_statement" {
                stmt.child_by_field_name("declaration").unwrap_or(stmt)
            } else {
                stmt
            };
            match target.kind() {
                "function_declaration"
                | "generator_function_declaration"
                | "class_declaration"
                | "abstract_class_declaration"
                | "interface_declaration"
                | "type_alias_declaration"
                | "enum_declaration"
                | "import_statement"
                | "comment" => {}
                "export_statement" => {}
                "lexical_declaration" | "variable_declaration" => {
                    out.extend(named_children(target).into_iter().filter(|d| {
                        d.kind() == "variable_declarator" && !self.bound.contains(&d.id())
                    }))
                }
                _ => out.push(target),
            }
        }
        out
    }
}

impl TsAdapter {
    fn grammar(&self) -> tree_sitter::Language {
        match self.0 {
            Flavor::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Flavor::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            Flavor::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
        }
    }

    /// Parses `source` and appends its symbols/imports to `facts`.
    pub(crate) fn parse_into(&self, source: &str, facts: &mut FileFacts) -> anyhow::Result<()> {
        let tree = common::parse_tree(&self.grammar(), source)?;
        let mut ctx = Ctx {
            source,
            symbols: vec![],
            imports: vec![],
            candidates: vec![],
            bound: vec![],
            known: known_functions(tree.root_node(), source),
        };
        for stmt in all_children(tree.root_node()) {
            ctx.statement(stmt);
        }
        ctx.callable_bindings();
        let module = ctx.body(&ctx.module_nodes(tree.root_node()));
        let mut handlers = vec![];
        ui_handlers(tree.root_node(), source, &mut handlers);
        common::apply_ui_handlers(&mut ctx.symbols, &handlers);
        facts.imports.extend(ctx.imports);
        facts.symbols.extend(ctx.symbols);
        if let Some(m) = common::module_symbol(module, facts.lines) {
            common::merge_module_symbol(&mut facts.symbols, m);
        }
        Ok(())
    }
}

/// `(event, identifiers)` from JSX `onClick={...}`, `x.addEventListener("click", ...)` and
/// `x.onclick = ...`.
fn ui_handlers(root: Node, source: &str, out: &mut Vec<(String, Vec<String>)>) {
    let mut pending = vec![root];
    while let Some(node) = pending.pop() {
        match node.kind() {
            "jsx_attribute" => {
                if let Some(name) = node.named_child(0)
                    && text(name, source).starts_with("on")
                    && let Some(event) = common::event_name(text(name, source))
                    && let Some(value) = node.named_child(1)
                {
                    out.push((event, common::handler_idents(text(value, source))));
                }
                continue;
            }
            "call_expression" => {
                if let Some(func) = node.child_by_field_name("function")
                    && func.kind() == "member_expression"
                    && field_text(func, "property", source).as_deref() == Some("addEventListener")
                    && let Some(args) = node.child_by_field_name("arguments")
                    && let [event, handler, ..] = named_children(args).as_slice()
                    && STRINGS.contains(&event.kind())
                {
                    let event = common::string_value(*event, source, INTERP, &[]);
                    out.push((
                        event.to_lowercase(),
                        common::handler_idents(text(*handler, source)),
                    ));
                }
            }
            "assignment_expression" => {
                if let Some(left) = node.child_by_field_name("left")
                    && left.kind() == "member_expression"
                    && let Some(prop) = field_text(left, "property", source)
                    && prop.starts_with("on")
                    && let Some(event) = common::event_name(&prop)
                    && let Some(right) = node.child_by_field_name("right")
                {
                    out.push((event, common::handler_idents(text(right, source))));
                }
            }
            _ => {}
        }
        pending.extend(named_children(node));
    }
}

impl Adapter for TsAdapter {
    fn language(&self) -> Language {
        match self.0 {
            Flavor::JavaScript => Language::JavaScript,
            _ => Language::TypeScript,
        }
    }

    fn extensions(&self) -> &[&str] {
        match self.0 {
            Flavor::TypeScript => &["ts", "mts", "cts"],
            Flavor::Tsx => &["tsx"],
            Flavor::JavaScript => &["js", "mjs", "cjs", "jsx"],
        }
    }

    fn parse(&self, rel_path: &str, source: &str, module: &str) -> anyhow::Result<FileFacts> {
        let mut facts = FileFacts {
            path: rel_path.to_string(),
            language: self.language(),
            module: module.to_string(),
            lines: common::line_count(source),
            imports: vec![],
            symbols: vec![],
        };
        self.parse_into(source, &mut facts)?;
        Ok(facts)
    }
}
