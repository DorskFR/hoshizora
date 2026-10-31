use hoshizora_model::{FileFacts, Import, Language, Param, Symbol, SymbolKind, Visibility};
use tree_sitter::Node;

use crate::Adapter;
use crate::common::{
    self, BodySpec, CallInfo, all_children, collapse_ws, field_text, has_child_kind, header,
    named_children, span, string_args, text,
};

pub struct PythonAdapter;

const STRINGS: &[&str] = &["string", "concatenated_string"];
const INTERP: &[&str] = &["interpolation"];
const SKIP: &[&str] = &["string_start", "string_end"];

const SPEC: BodySpec = BodySpec {
    loops: &[
        "for_statement",
        "while_statement",
        "list_comprehension",
        "set_comprehension",
        "dictionary_comprehension",
        "generator_expression",
    ],
    conditionals: &[
        "if_statement",
        "elif_clause",
        "else_clause",
        "conditional_expression",
        "case_clause",
        "except_clause",
        "match_statement",
    ],
    branches: &[
        "if_statement",
        "elif_clause",
        "conditional_expression",
        "except_clause",
    ],
    switches: &["match_statement"],
    arms: &["case_clause"],
    binary: &["boolean_operator"],
    closures: &["lambda"],
    early_exits: &["return_statement", "raise_statement"],
    awaits: &["await"],
    chain: &["call", "attribute", "subscript"],
    stop: &[
        "function_definition",
        "class_definition",
        "decorated_definition",
    ],
    ref_kinds: &["identifier", "attribute"],
    ref_containers: &["list", "tuple", "pair"],
    spawn: &[],
    call: call_info,
    expand: None,
};

fn string_arg(node: Node, source: &str) -> Option<String> {
    match node.kind() {
        k if STRINGS.contains(&k) => Some(common::string_value(node, source, INTERP, SKIP)),
        "parenthesized_expression" => named_children(node)
            .into_iter()
            .find_map(|c| string_arg(c, source)),
        "call" => {
            let func = node.child_by_field_name("function")?;
            if func.kind() != "attribute" || field_text(func, "attribute", source)? != "format" {
                return None;
            }
            let template = string_arg(func.child_by_field_name("object")?, source)?;
            Some(common::normalise_placeholders(&template))
        }
        "binary_operator" => {
            let op = text(node.child_by_field_name("operator")?, source);
            match op {
                "%" => string_arg(node.child_by_field_name("left")?, source)
                    .map(|t| common::normalise_placeholders(&t)),
                "+" => common::concat_value(node, source, "+", string_arg),
                _ => None,
            }
        }
        _ => None,
    }
}

fn call_info(node: Node, source: &str) -> Option<CallInfo> {
    if node.kind() != "call" {
        return None;
    }
    let func = node.child_by_field_name("function")?;
    let receiver = (func.kind() == "attribute")
        .then(|| field_text(func, "object", source))
        .flatten();
    Some(CallInfo::method(
        text(func, source),
        receiver,
        string_args(node.child_by_field_name("arguments"), source, string_arg),
    ))
}

fn docstring(body: Node, source: &str) -> Option<String> {
    let first = named_children(body).into_iter().next()?;
    if first.kind() != "expression_statement" {
        return None;
    }
    let s = named_children(first).into_iter().next()?;
    if s.kind() != "string" {
        return None;
    }
    let raw = common::string_value(s, source, &[], SKIP);
    let lines: Vec<&str> = raw.lines().collect();
    let indent = lines
        .iter()
        .skip(1)
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    let doc = lines
        .iter()
        .enumerate()
        .map(|(i, l)| {
            if i == 0 {
                l.trim()
            } else {
                l.get(indent..).unwrap_or("").trim_end()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string();
    (!doc.is_empty()).then_some(doc)
}

fn params(node: Node, source: &str) -> Vec<Param> {
    let Some(list) = node.child_by_field_name("parameters") else {
        return vec![];
    };
    named_children(list)
        .into_iter()
        .filter_map(|p| match p.kind() {
            "identifier" => Some(Param {
                name: text(p, source).into(),
                ty: None,
                default: None,
            }),
            "typed_parameter" => Some(Param {
                name: named_children(p)
                    .first()
                    .map(|n| text(*n, source).to_string())?,
                ty: field_text(p, "type", source),
                default: None,
            }),
            "default_parameter" | "typed_default_parameter" => Some(Param {
                name: field_text(p, "name", source)?,
                ty: field_text(p, "type", source),
                default: field_text(p, "value", source),
            }),
            "list_splat_pattern" | "dictionary_splat_pattern" => Some(Param {
                name: text(p, source).into(),
                ty: None,
                default: None,
            }),
            _ => None,
        })
        .collect()
}

fn visibility(name: &str) -> Visibility {
    if name.starts_with('_') && !(name.starts_with("__") && name.ends_with("__")) {
        Visibility::Private
    } else {
        Visibility::Public
    }
}

struct Ctx<'a> {
    source: &'a str,
    symbols: Vec<Symbol>,
    imports: Vec<Import>,
    /// Module-level `name = call(...)` awaiting a decision on whether the value is callable.
    candidates: Vec<(Node<'a>, Node<'a>, String)>,
    /// Statements emitted as symbols, excluded from `<module>`.
    bound: Vec<usize>,
}

impl<'a> Ctx<'a> {
    fn statement(&mut self, node: Node<'a>, parent: Option<&str>, in_class: bool) {
        match node.kind() {
            "decorated_definition" => {
                let decorators = named_children(node)
                    .into_iter()
                    .filter(|d| d.kind() == "decorator")
                    .map(|d| collapse_ws(text(d, self.source).trim_start_matches('@')))
                    .collect();
                if let Some(def) = node.child_by_field_name("definition") {
                    self.definition(def, parent, in_class, decorators);
                }
            }
            "function_definition" | "class_definition" => {
                self.definition(node, parent, in_class, vec![]);
            }
            "import_statement" | "import_from_statement" => self.import(node),
            "expression_statement" if parent.is_none() => {
                let Some(a) = named_children(node)
                    .into_iter()
                    .find(|n| n.kind() == "assignment")
                else {
                    return;
                };
                let Some(name) = field_text(a, "left", self.source) else {
                    return;
                };
                if let Some(v) = a.child_by_field_name("right")
                    && v.kind() == "call"
                    && a.child_by_field_name("left")
                        .is_some_and(|l| l.kind() == "identifier")
                {
                    self.candidates.push((node, v, name.clone()));
                }
                if name
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c == '_' || c.is_ascii_digit())
                    && name.chars().any(|c| c.is_ascii_uppercase())
                {
                    self.symbols.push(Symbol {
                        kind: SymbolKind::Constant,
                        local_name: name.clone(),
                        parent: None,
                        signature: collapse_ws(text(a, self.source)),
                        params: vec![],
                        returns: field_text(a, "type", self.source),
                        doc: None,
                        span: span(a),
                        is_async: false,
                        visibility: visibility(&name),
                        attributes: vec![],
                        implements: vec![],
                        body: None,
                        name,
                    });
                }
            }
            _ => {}
        }
    }

    fn definition(
        &mut self,
        node: Node<'a>,
        parent: Option<&str>,
        in_class: bool,
        attributes: Vec<String>,
    ) {
        let name = field_text(node, "name", self.source).unwrap_or_default();
        let local_name = parent.map_or_else(|| name.clone(), |p| format!("{p}.{name}"));
        let body = node.child_by_field_name("body");
        let is_class = node.kind() == "class_definition";
        let implements = node
            .child_by_field_name("superclasses")
            .map(|s| {
                named_children(s)
                    .iter()
                    .map(|n| common::strip_ws(text(*n, self.source)))
                    .collect()
            })
            .unwrap_or_default();
        self.symbols.push(Symbol {
            kind: if is_class {
                SymbolKind::Type
            } else if in_class {
                SymbolKind::Method
            } else {
                SymbolKind::Function
            },
            local_name: local_name.clone(),
            parent: parent.map(str::to_string),
            signature: header(node, self.source, "body"),
            params: if is_class {
                vec![]
            } else {
                params(node, self.source)
            },
            returns: field_text(node, "return_type", self.source),
            doc: body.and_then(|b| docstring(b, self.source)),
            span: span(node),
            is_async: has_child_kind(node, "async"),
            visibility: visibility(&name),
            attributes,
            implements,
            body: (!is_class)
                .then(|| body.map(|b| common::analyze_body(b, self.source, &SPEC)))
                .flatten(),
            name,
        });
        if let Some(body) = body {
            for child in named_children(body) {
                self.statement(child, Some(&local_name), is_class);
            }
        }
    }

    fn import(&mut self, node: Node) {
        let line = common::line(node);
        let src = self.source;
        if node.kind() == "import_from_statement" {
            let path = field_text(node, "module_name", src).unwrap_or_default();
            let names = node
                .children_by_field_name("name", &mut node.walk())
                .map(|n| collapse_ws(text(n, src)))
                .collect::<Vec<_>>();
            let names = if names.is_empty() && has_child_kind(node, "wildcard_import") {
                vec!["*".to_string()]
            } else {
                names
            };
            self.imports.push(Import {
                path,
                alias: None,
                names,
                line,
            });
            return;
        }
        for n in node.children_by_field_name("name", &mut node.walk()) {
            if n.kind() == "aliased_import" {
                self.imports.push(Import {
                    path: field_text(n, "name", src).unwrap_or_default(),
                    alias: field_text(n, "alias", src),
                    names: vec![],
                    line,
                });
            } else {
                self.imports.push(Import {
                    path: collapse_ws(text(n, src)),
                    alias: None,
                    names: vec![],
                    line,
                });
            }
        }
    }
}

impl Ctx<'_> {
    fn callable_bindings(&mut self) {
        let src = self.source;
        let fn_names: Vec<String> = self
            .symbols
            .iter()
            .filter(|s| s.kind == SymbolKind::Function)
            .map(|s| s.name.clone())
            .collect();
        for (stmt, call, name) in std::mem::take(&mut self.candidates) {
            let args = call
                .child_by_field_name("arguments")
                .map(named_children)
                .unwrap_or_default();
            let lambda = args.iter().copied().find(|a| a.kind() == "lambda");
            let by_name = args
                .iter()
                .any(|a| a.kind() == "identifier" && fn_names.iter().any(|n| n == text(*a, src)));
            if lambda.is_none() && !by_name {
                continue;
            }
            self.bound.push(stmt.id());
            let end = lambda
                .and_then(|l| l.child_by_field_name("body"))
                .map_or(stmt.end_byte(), |b| b.start_byte());
            let signature = collapse_ws(&src[stmt.start_byte()..end]);
            self.symbols.push(Symbol {
                kind: SymbolKind::Function,
                local_name: name.clone(),
                parent: None,
                signature: signature.trim_end_matches([' ', ':']).to_string(),
                params: vec![],
                returns: None,
                doc: None,
                span: span(stmt),
                is_async: false,
                visibility: visibility(&name),
                attributes: vec![],
                implements: vec![],
                body: Some(common::analyze_nodes(&[call], src, &SPEC)),
                name,
            });
        }
    }
}

impl Adapter for PythonAdapter {
    fn language(&self) -> Language {
        Language::Python
    }

    fn extensions(&self) -> &[&str] {
        &["py"]
    }

    fn parse(&self, rel_path: &str, source: &str, module: &str) -> anyhow::Result<FileFacts> {
        let tree = common::parse_tree(&tree_sitter_python::LANGUAGE.into(), source)?;
        let mut ctx = Ctx {
            source,
            symbols: vec![],
            imports: vec![],
            candidates: vec![],
            bound: vec![],
        };
        for stmt in all_children(tree.root_node()) {
            ctx.statement(stmt, None, false);
        }
        ctx.callable_bindings();
        let top: Vec<Node> = named_children(tree.root_node())
            .into_iter()
            .filter(|n| !ctx.bound.contains(&n.id()))
            .collect();
        if let Some(m) = common::module_symbol(
            common::analyze_nodes(&top, source, &SPEC),
            common::line_count(source),
        ) {
            ctx.symbols.push(m);
        }
        Ok(FileFacts {
            path: rel_path.to_string(),
            language: Language::Python,
            module: module.to_string(),
            lines: common::line_count(source),
            imports: ctx.imports,
            symbols: ctx.symbols,
        })
    }
}
