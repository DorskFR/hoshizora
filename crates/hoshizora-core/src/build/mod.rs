pub(crate) mod analysis;
mod effects;
mod packages;
mod resolve;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use hoshizora_model::{
    Confidence, Edge, EdgeKind, Effects, FileFacts, FunctionInfo, GRAPH_VERSION, Graph, GraphMeta,
    Language, Node, NodeId, NodeKind, Span, Symbol, SymbolKind,
};

use crate::packs::Packs;

pub(crate) struct FnRec {
    pub node: usize,
    pub file: usize,
    pub sym: usize,
    /// Node index of the enclosing type, for methods.
    pub parent_type: Option<usize>,
}

#[derive(Debug, Clone)]
pub(crate) struct ImportTarget {
    /// Imported path as written.
    pub path: String,
    /// File index when the import resolves inside the repo.
    pub file: Option<usize>,
    /// Dependency node id (`ext:<pkg>`) when it does not.
    pub dependency: Option<NodeId>,
}

#[derive(Default)]
pub(crate) struct FileScope {
    /// Local binding (alias, imported name, last path segment) → import.
    pub bindings: HashMap<String, usize>,
    pub imports: Vec<ImportTarget>,
    /// Imports that bring every name of a module into scope (`use x::*`, `from x import *`).
    pub globs: Vec<usize>,
}

pub(crate) struct Ctx<'a> {
    pub files: &'a [FileFacts],
    pub packs: &'a Packs,
    pub nodes: Vec<Node>,
    pub node_index: HashMap<NodeId, usize>,
    pub edges: Vec<Edge>,
    /// Node indices of `edges[i]`'s endpoints.
    pub edge_ends: Vec<(usize, usize)>,
    pub edge_set: HashSet<(usize, usize, EdgeKind, Option<u32>)>,
    pub fns: Vec<FnRec>,
    /// Number of call sites per function that resolve to the function itself.
    pub self_calls: Vec<u32>,
    pub file_nodes: Vec<usize>,
    pub package_of_file: Vec<usize>,
    pub scopes: Vec<FileScope>,
    /// Per package: itself plus the packages its files import from.
    pub pkg_allowed: Vec<HashSet<usize>>,
    pub rule_sets: Vec<effects::RuleSet<'a>>,
    /// Per file: local name → function.
    pub fn_by_file_local: Vec<HashMap<String, usize>>,
    /// Per file: short name → functions.
    pub fn_by_file_name: Vec<HashMap<String, Vec<usize>>>,
    /// Package node → short name → functions.
    pub fn_by_package_name: HashMap<usize, HashMap<String, Vec<usize>>>,
    pub fn_by_name: HashMap<String, Vec<usize>>,
    /// Type name → method name → functions, for receiver-typed resolution.
    pub fn_by_type_method: HashMap<String, HashMap<String, Vec<usize>>>,
    /// Per file: local name → type node.
    pub type_by_file_local: Vec<HashMap<String, usize>>,
    pub type_by_name: HashMap<String, Vec<usize>>,
    /// Type node → field name → declared type or initialiser (`useX()`, `Store`).
    pub type_fields: HashMap<usize, HashMap<String, String>>,
    /// Package name (dashes as underscores) → package node.
    pub package_names: HashMap<String, usize>,
    /// Normalised module key (`a/b/c`) → files.
    pub module_files: HashMap<String, Vec<usize>>,
    pub counters: BTreeMap<&'static str, u32>,
    pub warnings: Vec<String>,
}

impl<'a> Ctx<'a> {
    pub fn count(&mut self, key: &'static str) {
        *self.counters.entry(key).or_insert(0) += 1;
    }

    pub fn add_node(&mut self, node: Node) -> usize {
        if let Some(&i) = self.node_index.get(&node.id) {
            return i;
        }
        let i = self.nodes.len();
        self.node_index.insert(node.id.clone(), i);
        self.nodes.push(node);
        i
    }

    pub fn add_edge(&mut self, from: usize, to: usize, edge: Edge) -> bool {
        if !self.edge_set.insert((from, to, edge.kind, edge.line)) {
            return false;
        }
        self.edges.push(edge);
        self.edge_ends.push((from, to));
        true
    }

    pub fn simple_edge(&mut self, from: usize, to: usize, kind: EdgeKind, confidence: Confidence) {
        let edge = Edge::new(
            self.nodes[from].id.clone(),
            self.nodes[to].id.clone(),
            kind,
            confidence,
        );
        self.add_edge(from, to, edge);
    }

    pub fn synthetic(&mut self, kind: NodeKind, id: String, name: String) -> usize {
        if let Some(&i) = self.node_index.get(&id) {
            return i;
        }
        self.add_node(Node {
            id,
            kind,
            qualified_name: name.clone(),
            name,
            language: None,
            file: None,
            span: None,
            parent: None,
            function: None,
            effects: Effects::default(),
            entrypoint: None,
            dead: false,
            lines: 0,
        })
    }

    pub fn symbol(&self, f: usize) -> &'a Symbol {
        &self.files[self.fns[f].file].symbols[self.fns[f].sym]
    }
}

pub fn build_graph(root: &Path, files: Vec<FileFacts>, packs: &Packs) -> Graph {
    let mut files = files;
    files.sort_by(|a, b| a.path.cmp(&b.path));
    files.dedup_by(|a, b| a.path == b.path);

    let mut ctx = Ctx {
        files: &files,
        packs,
        nodes: Vec::new(),
        node_index: HashMap::new(),
        edges: Vec::new(),
        edge_ends: Vec::new(),
        edge_set: HashSet::new(),
        fns: Vec::new(),
        self_calls: Vec::new(),
        file_nodes: Vec::with_capacity(files.len()),
        package_of_file: Vec::with_capacity(files.len()),
        scopes: Vec::with_capacity(files.len()),
        pkg_allowed: Vec::new(),
        rule_sets: Vec::with_capacity(files.len()),
        fn_by_file_local: vec![HashMap::new(); files.len()],
        fn_by_file_name: vec![HashMap::new(); files.len()],
        fn_by_package_name: HashMap::new(),
        fn_by_name: HashMap::new(),
        fn_by_type_method: HashMap::new(),
        type_by_file_local: vec![HashMap::new(); files.len()],
        type_by_name: HashMap::new(),
        type_fields: HashMap::new(),
        package_names: HashMap::new(),
        module_files: HashMap::new(),
        counters: BTreeMap::new(),
        warnings: Vec::new(),
    };

    packages::build(&mut ctx);
    build_files_and_symbols(&mut ctx);
    resolve::build_scopes(&mut ctx);
    effects::build_rule_sets(&mut ctx);
    effects::routes(&mut ctx);
    resolve::calls(&mut ctx);
    effects::effects(&mut ctx);
    analysis::entrypoints(&mut ctx);
    analysis::complexity(&mut ctx);
    analysis::transitive(&mut ctx);
    analysis::dead(&mut ctx);

    let mut languages: Vec<Language> = files.iter().map(|f| f.language).collect();
    languages.sort();
    languages.dedup();

    let mut warnings = ctx.warnings;
    for (key, n) in &ctx.counters {
        warnings.push(format!("{key}: {n}"));
    }

    Graph {
        meta: GraphMeta {
            version: GRAPH_VERSION,
            root: root.to_string_lossy().into_owned(),
            generated_at: crate::store::now_rfc3339(),
            languages,
            files: files.len() as u32,
            warnings,
        },
        nodes: ctx.nodes,
        edges: ctx.edges,
    }
}

pub(crate) fn fn_id(path: &str, local_name: &str) -> NodeId {
    format!("fn:{path}#{local_name}")
}

pub(crate) fn type_id(path: &str, local_name: &str) -> NodeId {
    format!("type:{path}#{local_name}")
}

fn build_files_and_symbols(ctx: &mut Ctx) {
    for (fi, file) in ctx.files.iter().enumerate() {
        let pkg = ctx.package_of_file[fi];
        let pkg_id = ctx.nodes[pkg].id.clone();
        let file_node = ctx.add_node(Node {
            id: format!("file:{}", file.path),
            kind: NodeKind::File,
            name: file
                .path
                .rsplit('/')
                .next()
                .unwrap_or(&file.path)
                .to_owned(),
            qualified_name: file.path.clone(),
            language: Some(file.language),
            file: Some(file.path.clone()),
            span: Some(Span {
                start_line: 1,
                end_line: file.lines.max(1),
            }),
            parent: Some(pkg_id),
            function: None,
            effects: Effects::default(),
            entrypoint: None,
            dead: false,
            lines: file.lines,
        });
        ctx.file_nodes.push(file_node);
        ctx.simple_edge(pkg, file_node, EdgeKind::Contains, Confidence::Exact);

        for key in packages::module_keys(file) {
            ctx.module_files.entry(key).or_default().push(fi);
        }

        for sym in &file.symbols {
            if matches!(sym.kind, SymbolKind::Type | SymbolKind::Interface) {
                let id = type_id(&file.path, &sym.local_name);
                let n = ctx.add_node(Node {
                    id,
                    kind: NodeKind::Type,
                    name: sym.name.clone(),
                    qualified_name: qualified(file, &sym.local_name),
                    language: Some(file.language),
                    file: Some(file.path.clone()),
                    span: Some(sym.span),
                    parent: Some(ctx.nodes[file_node].id.clone()),
                    function: None,
                    effects: Effects::default(),
                    entrypoint: None,
                    dead: false,
                    lines: span_lines(sym.span),
                });
                ctx.type_by_file_local[fi].insert(sym.local_name.clone(), n);
                ctx.type_by_name
                    .entry(sym.name.clone())
                    .or_default()
                    .push(n);
                let fields: HashMap<String, String> = sym
                    .params
                    .iter()
                    .filter_map(|p| Some((p.name.clone(), p.ty.clone()?)))
                    .collect();
                if !fields.is_empty() {
                    ctx.type_fields.insert(n, fields);
                }
                ctx.simple_edge(file_node, n, EdgeKind::Contains, Confidence::Exact);
            }
        }

        for (si, sym) in file.symbols.iter().enumerate() {
            if !matches!(sym.kind, SymbolKind::Function | SymbolKind::Method) {
                continue;
            }
            let parent_type = sym
                .parent
                .as_ref()
                .and_then(|p| ctx.type_by_file_local[fi].get(p).copied());
            let parent_id = match parent_type {
                Some(t) => ctx.nodes[t].id.clone(),
                None => ctx.nodes[file_node].id.clone(),
            };
            let body = sym.body.clone().unwrap_or_default();
            let node = Node {
                id: fn_id(&file.path, &sym.local_name),
                kind: NodeKind::Function,
                name: sym.name.clone(),
                qualified_name: qualified(file, &sym.local_name),
                language: Some(file.language),
                file: Some(file.path.clone()),
                span: Some(sym.span),
                parent: Some(parent_id),
                function: Some(FunctionInfo {
                    signature: sym.signature.clone(),
                    params: sym.params.clone(),
                    returns: sym.returns.clone(),
                    doc: sym.doc.clone(),
                    is_async: sym.is_async,
                    is_method: sym.kind == SymbolKind::Method || sym.parent.is_some(),
                    visibility: sym.visibility,
                    attributes: sym.attributes.clone(),
                    cyclomatic: body.cyclomatic.max(1),
                    branches: body.branches,
                    loops: body.loops,
                    max_loop_depth: body.max_loop_depth,
                    recursive: false,
                    complexity: hoshizora_model::Complexity {
                        time: "unknown".into(),
                        space: "unknown".into(),
                        basis: Vec::new(),
                    },
                    fan_in: 0,
                    fan_out: 0,
                }),
                effects: Effects::default(),
                entrypoint: None,
                dead: false,
                lines: span_lines(sym.span),
            };
            if ctx.node_index.contains_key(&node.id) {
                ctx.count("duplicate function symbols skipped");
                continue;
            }
            let n = ctx.add_node(node);
            let f = ctx.fns.len();
            ctx.fns.push(FnRec {
                node: n,
                file: fi,
                sym: si,
                parent_type,
            });
            ctx.fn_by_file_local[fi].insert(sym.local_name.clone(), f);
            ctx.fn_by_file_name[fi]
                .entry(sym.name.clone())
                .or_default()
                .push(f);
            ctx.fn_by_package_name
                .entry(pkg)
                .or_default()
                .entry(sym.name.clone())
                .or_default()
                .push(f);
            ctx.fn_by_name.entry(sym.name.clone()).or_default().push(f);
            if let Some(p) = &sym.parent {
                let type_name = p.rsplit(['.', ':']).next().unwrap_or(p).to_owned();
                ctx.fn_by_type_method
                    .entry(type_name)
                    .or_default()
                    .entry(sym.name.clone())
                    .or_default()
                    .push(f);
            }
            let container = parent_type.unwrap_or(file_node);
            ctx.simple_edge(container, n, EdgeKind::Contains, Confidence::Exact);
        }

        for sym in &file.symbols {
            if !matches!(sym.kind, SymbolKind::Type | SymbolKind::Interface) {
                continue;
            }
            let Some(&from) = ctx.type_by_file_local[fi].get(&sym.local_name) else {
                continue;
            };
            for imp in &sym.implements {
                let short = imp.rsplit(['.', ':']).next().unwrap_or(imp);
                let targets = ctx.type_by_file_local[fi]
                    .get(imp)
                    .map(|&t| vec![t])
                    .or_else(|| ctx.type_by_name.get(short).cloned())
                    .unwrap_or_default();
                if targets.len() == 1 {
                    let confidence = if ctx.type_by_file_local[fi].contains_key(imp) {
                        Confidence::Exact
                    } else {
                        Confidence::Name
                    };
                    ctx.simple_edge(from, targets[0], EdgeKind::Implements, confidence);
                }
            }
        }
    }
}

pub(crate) fn qualified(file: &FileFacts, local_name: &str) -> String {
    if file.module.is_empty() {
        return local_name.to_owned();
    }
    let sep = match file.language {
        Language::Rust => "::",
        Language::Python => ".",
        Language::Go | Language::TypeScript | Language::JavaScript => ".",
    };
    format!("{}{sep}{local_name}", file.module)
}

pub(crate) fn span_lines(span: Span) -> u32 {
    span.end_line.saturating_sub(span.start_line) + 1
}
