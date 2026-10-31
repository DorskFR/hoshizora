use hoshizora_model::{FileFacts, Import, Language, Param, Symbol, SymbolKind, Visibility};
use tree_sitter::Node;

use crate::Adapter;
use crate::common::{
    self, BodySpec, CallInfo, all_children, clean_comment_lines, collapse_ws, field_text, header,
    named_children, preceding_comments, span, string_args, strip_generics, text,
};

pub struct GoAdapter;

const STRINGS: &[&str] = &["interpreted_string_literal", "raw_string_literal"];

const SPEC: BodySpec = BodySpec {
    loops: &["for_statement"],
    conditionals: &[
        "if_statement",
        "expression_case",
        "type_case",
        "default_case",
        "communication_case",
    ],
    branches: &["if_statement"],
    switches: &[
        "expression_switch_statement",
        "type_switch_statement",
        "select_statement",
    ],
    arms: &[
        "expression_case",
        "type_case",
        "default_case",
        "communication_case",
    ],
    binary: &["binary_expression"],
    closures: &["func_literal"],
    early_exits: &["return_statement"],
    awaits: &[],
    chain: &[],
    stop: &[],
    ref_kinds: &["identifier", "selector_expression"],
    ref_containers: &["literal_element", "keyed_element"],
    spawn: &["go_statement"],
    call: call_info,
    expand: None,
};

fn string_arg(node: Node, source: &str) -> Option<String> {
    match node.kind() {
        k if STRINGS.contains(&k) => Some(common::string_value(node, source, &[], &[])),
        "parenthesized_expression" => named_children(node)
            .into_iter()
            .find_map(|c| string_arg(c, source)),
        "binary_expression" => common::concat_value(node, source, "+", string_arg),
        "call_expression" => {
            let func = text(node.child_by_field_name("function")?, source);
            if !matches!(func, "fmt.Sprintf" | "fmt.Sprint" | "fmt.Errorf") {
                return None;
            }
            let args = node.child_by_field_name("arguments")?;
            let template = named_children(args)
                .into_iter()
                .find_map(|a| string_arg(a, source))?;
            Some(common::normalise_placeholders(&template))
        }
        _ => None,
    }
}

fn call_info(node: Node, source: &str) -> Option<CallInfo> {
    if node.kind() != "call_expression" {
        return None;
    }
    let func = node.child_by_field_name("function")?;
    if func.kind() == "func_literal" {
        return None;
    }
    let receiver = (func.kind() == "selector_expression")
        .then(|| field_text(func, "operand", source))
        .flatten();
    Some(CallInfo::method(
        text(func, source),
        receiver,
        string_args(node.child_by_field_name("arguments"), source, string_arg),
    ))
}

fn doc(node: Node, source: &str) -> Option<String> {
    let lines: Vec<&str> = preceding_comments(node, &["comment"], &[])
        .into_iter()
        .map(|c| text(c, source))
        .collect();
    clean_comment_lines(&lines, &["//", "/*", "*"])
}

fn param_list(list: Option<Node>, source: &str) -> Vec<Param> {
    let Some(list) = list else { return vec![] };
    let mut out = vec![];
    for p in named_children(list) {
        let ty = field_text(p, "type", source).map(|t| {
            if p.kind() == "variadic_parameter_declaration" {
                format!("...{t}")
            } else {
                t
            }
        });
        let names: Vec<String> = p
            .children_by_field_name("name", &mut p.walk())
            .map(|n| text(n, source).to_string())
            .collect();
        if names.is_empty() {
            out.push(Param {
                name: String::new(),
                ty: ty.clone(),
                default: None,
            });
        }
        for name in names {
            out.push(Param {
                name,
                ty: ty.clone(),
                default: None,
            });
        }
    }
    out
}

fn visibility(name: &str) -> Visibility {
    if name.chars().next().is_some_and(char::is_uppercase) {
        Visibility::Public
    } else {
        Visibility::Private
    }
}

struct Ctx<'a> {
    source: &'a str,
    symbols: Vec<Symbol>,
    imports: Vec<Import>,
}

impl Ctx<'_> {
    fn function(&mut self, node: Node) {
        let src = self.source;
        let name = field_text(node, "name", src).unwrap_or_default();
        let recv = node.child_by_field_name("receiver").and_then(|r| {
            named_children(r)
                .first()
                .and_then(|p| p.child_by_field_name("type"))
                .map(|t| strip_generics(text(t, src).trim_start_matches('*')))
        });
        let local_name = recv
            .as_ref()
            .map_or_else(|| name.clone(), |r| format!("{r}.{name}"));
        self.symbols.push(Symbol {
            kind: if recv.is_some() {
                SymbolKind::Method
            } else {
                SymbolKind::Function
            },
            local_name,
            parent: recv,
            signature: header(node, src, "body"),
            params: param_list(node.child_by_field_name("parameters"), src),
            returns: field_text(node, "result", src),
            doc: doc(node, src),
            span: span(node),
            is_async: false,
            visibility: visibility(&name),
            attributes: vec![],
            implements: vec![],
            body: node
                .child_by_field_name("body")
                .map(|b| common::analyze_body(b, src, &SPEC)),
            name,
        });
    }

    fn spec(&mut self, decl: Node, spec: Node, kind: SymbolKind) {
        let src = self.source;
        let name = field_text(spec, "name", src).unwrap_or_default();
        let ty = spec.child_by_field_name("type");
        let is_interface = ty.is_some_and(|t| t.kind() == "interface_type");
        let doc_node = if named_children(decl).len() == 1 {
            decl
        } else {
            spec
        };
        let keyword = if kind == SymbolKind::Constant {
            "const"
        } else {
            "type"
        };
        let signature = match ty.map(|t| t.kind()) {
            Some("struct_type") => format!("type {name} struct"),
            Some("interface_type") => format!("type {name} interface"),
            _ => format!("{keyword} {}", collapse_ws(text(spec, src))),
        };
        self.symbols.push(Symbol {
            kind: if is_interface {
                SymbolKind::Interface
            } else {
                kind
            },
            local_name: name.clone(),
            parent: None,
            signature,
            params: vec![],
            returns: None,
            doc: doc(doc_node, src),
            span: span(spec),
            is_async: false,
            visibility: visibility(&name),
            attributes: vec![],
            implements: vec![],
            body: None,
            name,
        });
    }

    fn item(&mut self, node: Node) {
        match node.kind() {
            "function_declaration" | "method_declaration" => self.function(node),
            "type_declaration" => {
                for s in named_children(node)
                    .into_iter()
                    .filter(|s| s.kind() == "type_spec")
                {
                    self.spec(node, s, SymbolKind::Type);
                }
            }
            "const_declaration" => {
                for s in named_children(node)
                    .into_iter()
                    .filter(|s| s.kind() == "const_spec")
                {
                    self.spec(node, s, SymbolKind::Constant);
                }
            }
            "import_declaration" => {
                let src = self.source;
                let mut specs = vec![];
                for c in named_children(node) {
                    match c.kind() {
                        "import_spec" => specs.push(c),
                        "import_spec_list" => specs.extend(
                            named_children(c)
                                .into_iter()
                                .filter(|s| s.kind() == "import_spec"),
                        ),
                        _ => {}
                    }
                }
                for s in specs {
                    self.imports.push(Import {
                        path: s
                            .child_by_field_name("path")
                            .map(|p| common::string_value(p, src, &[], &[]))
                            .unwrap_or_default(),
                        alias: field_text(s, "name", src),
                        names: vec![],
                        line: common::line(s),
                    });
                }
            }
            _ => {}
        }
    }
}

impl Adapter for GoAdapter {
    fn language(&self) -> Language {
        Language::Go
    }

    fn extensions(&self) -> &[&str] {
        &["go"]
    }

    fn parse(&self, rel_path: &str, source: &str, module: &str) -> anyhow::Result<FileFacts> {
        let tree = common::parse_tree(&tree_sitter_go::LANGUAGE.into(), source)?;
        let mut ctx = Ctx {
            source,
            symbols: vec![],
            imports: vec![],
        };
        for item in all_children(tree.root_node()) {
            ctx.item(item);
        }
        let initialisers: Vec<Node> = named_children(tree.root_node())
            .into_iter()
            .filter(|n| n.kind() == "var_declaration")
            .flat_map(|d| {
                let mut specs = named_children(d);
                if let Some(list) = specs.iter().find(|s| s.kind() == "var_spec_list") {
                    specs = named_children(*list);
                }
                specs
            })
            .filter_map(|spec| spec.child_by_field_name("value"))
            .collect();
        if let Some(m) = common::module_symbol(
            common::analyze_nodes(&initialisers, source, &SPEC),
            common::line_count(source),
        ) {
            ctx.symbols.push(m);
        }
        Ok(FileFacts {
            path: rel_path.to_string(),
            language: Language::Go,
            module: module.to_string(),
            lines: common::line_count(source),
            imports: ctx.imports,
            symbols: ctx.symbols,
        })
    }
}
