//! Entrypoints, complexity estimates, transitive effects and dead-code marking.

use std::collections::{HashMap, HashSet, VecDeque};

use hoshizora_model::{
    Complexity, EdgeKind, EffectKind, EntrypointKind, Language, SymbolKind, Visibility,
};

use super::Ctx;
use super::resolve::{plausible_call, trusted_call};

/// Edge kinds along which control and effects flow.
pub const FLOW_KINDS: &[EdgeKind] = &[
    EdgeKind::Calls,
    EdgeKind::Spawns,
    EdgeKind::Handles,
    EdgeKind::Requests,
];

pub const ALL_EFFECTS: [EffectKind; 14] = [
    EffectKind::Network,
    EffectKind::DbRead,
    EffectKind::DbWrite,
    EffectKind::DbDelete,
    EffectKind::FsRead,
    EffectKind::FsWrite,
    EffectKind::Process,
    EffectKind::Spawn,
    EffectKind::Join,
    EffectKind::Lock,
    EffectKind::Publish,
    EffectKind::Subscribe,
    EffectKind::Sleep,
    EffectKind::Blocking,
];

pub fn effect_bit(kind: EffectKind) -> u16 {
    1 << ALL_EFFECTS.iter().position(|k| *k == kind).unwrap_or(0)
}

pub(crate) fn is_test_file(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.starts_with("test_")
        || name.ends_with("_test.py")
        || name.ends_with("_test.go")
        || name.ends_with(".test.ts")
        || name.ends_with(".test.tsx")
        || name.ends_with(".test.js")
        || name.ends_with(".spec.ts")
        || name.ends_with(".spec.js")
        || path.starts_with("tests/")
        || path.contains("/tests/")
        || path.contains("/__tests__/")
}

fn is_ui_file(path: &str) -> bool {
    path.ends_with(".svelte")
        || path.ends_with(".tsx")
        || path.ends_with(".jsx")
        || path.ends_with(".vue")
}

pub(crate) fn entrypoints(ctx: &mut Ctx) {
    let mut lib_packages: HashSet<usize> = HashSet::new();
    for (fi, file) in ctx.files.iter().enumerate() {
        if file.language == Language::Rust
            && (file.path == "src/lib.rs" || file.path.ends_with("/src/lib.rs"))
        {
            lib_packages.insert(ctx.package_of_file[fi]);
        }
    }
    let mut handled: HashSet<usize> = HashSet::new();
    for (e, &(_, to)) in ctx.edges.iter().zip(&ctx.edge_ends) {
        if e.kind == EdgeKind::Handles {
            handled.insert(to);
        }
    }

    for f in 0..ctx.fns.len() {
        let sym = ctx.symbol(f);
        let fi = ctx.fns[f].file;
        let file = &ctx.files[fi];
        let node = ctx.fns[f].node;
        let is_main_attr = sym.attributes.iter().any(|a| {
            let a = a.split('(').next().unwrap_or(a);
            matches!(
                a,
                "tokio::main" | "async_std::main" | "actix_web::main" | "main"
            )
        });
        let is_test_attr = sym.attributes.iter().any(|a| {
            let a = a.split('(').next().unwrap_or(a);
            matches!(
                a,
                "test"
                    | "tokio::test"
                    | "async_std::test"
                    | "actix_web::test"
                    | "rstest"
                    | "sqlx::test"
                    | "pytest.fixture"
            ) || a.starts_with("pytest.mark")
        });
        let is_ui_attr = sym.attributes.iter().any(|a| a.starts_with("ui:"));
        let kind = if is_main_attr
            || (matches!(sym.name.as_str(), "main" | "<module>") && sym.parent.is_none())
        {
            Some(EntrypointKind::Main)
        } else if is_test_attr
            || sym.name.starts_with("test_")
            || (file.language == Language::Go
                && file.path.ends_with("_test.go")
                && sym.name.starts_with("Test"))
            || (is_test_file(&file.path) && file.language != Language::Go)
        {
            Some(EntrypointKind::Test)
        } else if is_ui_attr {
            Some(EntrypointKind::UiHandler)
        } else if handled.contains(&node) {
            Some(EntrypointKind::HttpHandler)
        } else if is_ui_file(&file.path)
            && (sym.visibility == Visibility::Public
                || sym.name.starts_with("handle")
                || sym.name.starts_with("on"))
        {
            Some(EntrypointKind::UiHandler)
        } else if sym.visibility == Visibility::Public
            && is_export_file(
                file.language,
                &file.path,
                lib_packages.contains(&ctx.package_of_file[fi]),
            )
        {
            Some(EntrypointKind::Export)
        } else {
            None
        };
        ctx.nodes[node].entrypoint = kind;
    }
}

fn is_export_file(language: Language, path: &str, in_lib_crate: bool) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    match language {
        Language::Rust => in_lib_crate,
        Language::Python => name == "__init__.py",
        Language::TypeScript | Language::JavaScript => {
            matches!(name, "index.ts" | "index.js" | "index.mjs" | "mod.ts")
        }
        Language::Go => false,
    }
}

#[derive(Clone)]
struct CallInfo {
    target: usize,
    loop_depth: u32,
    trusted: bool,
    /// Works on the current loop item only: its callee's `n` is that item, not the input.
    per_item: bool,
}

const SORT_NAMES: &[&str] = &[
    "sort",
    "sort_by",
    "sort_by_key",
    "sort_unstable",
    "sort_unstable_by",
    "sort_unstable_by_key",
    "sorted",
    "toSorted",
    "sort.Slice",
    "sort.Sort",
    "sort.Strings",
    "sort.Ints",
    "slices.Sort",
    "slices.SortFunc",
    "sort_values",
    "order_by",
];

const ALLOC_NAMES: &[&str] = &[
    "push",
    "push_back",
    "push_str",
    "append",
    "extend",
    "extend_from_slice",
    "collect",
    "insert",
    "to_vec",
    "to_owned",
    "to_string",
    "clone",
    "format",
    "vec",
    "concat",
    "add",
    "set",
    "unshift",
    "splice",
    "Vec::new",
    "String::new",
    "HashMap::new",
    "Box::new",
    "Arc::new",
    "make",
    "list",
    "dict",
    "copy",
    "deepcopy",
];

pub(crate) fn complexity(ctx: &mut Ctx) {
    let node_to_fn: HashMap<usize, usize> = ctx
        .fns
        .iter()
        .enumerate()
        .map(|(f, r)| (r.node, f))
        .collect();
    let mut out: Vec<Vec<CallInfo>> = vec![Vec::new(); ctx.fns.len()];
    let mut fan_in: Vec<HashSet<usize>> = vec![HashSet::new(); ctx.fns.len()];
    let mut fan_out: Vec<HashSet<usize>> = vec![HashSet::new(); ctx.fns.len()];
    for (e, &(from, to)) in ctx.edges.iter().zip(&ctx.edge_ends) {
        if !matches!(e.kind, EdgeKind::Calls | EdgeKind::Spawns) {
            continue;
        }
        let (Some(&ff), Some(&tf)) = (node_to_fn.get(&from), node_to_fn.get(&to)) else {
            continue;
        };
        out[ff].push(CallInfo {
            target: tf,
            loop_depth: e.loop_depth,
            trusted: trusted_call(e),
            per_item: e.attrs.contains_key("per_item"),
        });
        fan_out[ff].insert(tf);
        fan_in[tf].insert(ff);
    }

    let mut memo: Vec<Option<(u32, bool)>> = vec![None; ctx.fns.len()];
    let mut on_stack = vec![false; ctx.fns.len()];
    let mut results: Vec<(Complexity, bool)> = Vec::with_capacity(ctx.fns.len());
    for f in 0..ctx.fns.len() {
        let (exp, exponential) = exponent(ctx, f, &out, &mut memo, &mut on_stack);
        let (complexity, recursive) = describe(ctx, f, &out, exp, exponential, &memo);
        results.push((complexity, recursive));
    }

    for (f, (complexity, recursive)) in results.into_iter().enumerate() {
        let node = ctx.fns[f].node;
        if let Some(info) = ctx.nodes[node].function.as_mut() {
            info.complexity = complexity;
            info.recursive = recursive;
            info.fan_in = fan_in[f].len() as u32;
            info.fan_out = fan_out[f].len() as u32;
        }
    }
}

/// Polynomial exponent of a function's running time, with an `exponential` flag for branching
/// recursion. Only calls inside a loop, through a trusted edge, on more than the current loop
/// item, add their callee's exponent; cycles other than direct self-calls contribute nothing
/// (treated as unknown).
fn exponent(
    ctx: &Ctx,
    f: usize,
    out: &[Vec<CallInfo>],
    memo: &mut Vec<Option<(u32, bool)>>,
    on_stack: &mut Vec<bool>,
) -> (u32, bool) {
    if let Some(r) = memo[f] {
        return r;
    }
    if on_stack[f] {
        return (0, false);
    }
    on_stack[f] = true;
    let sym = ctx.symbol(f);
    let body = sym.body.as_ref();
    let mut exp = body.map(|b| b.max_loop_depth).unwrap_or(0);
    let mut exponential = false;

    for call in &out[f] {
        if call.target == f || call.loop_depth == 0 || !call.trusted {
            continue;
        }
        let (k, e) = exponent(ctx, call.target, out, memo, on_stack);
        exponential |= e;
        if !call.per_item {
            exp = exp.max(k + call.loop_depth);
        }
    }
    match ctx.self_calls[f].len() {
        0 => {}
        _ if branching(&ctx.self_calls[f]) => exponential = true,
        _ => exp = exp.max(1),
    }
    if let Some(b) = body
        && b.calls.iter().any(|c| {
            SORT_NAMES.contains(&c.name.as_str()) || SORT_NAMES.contains(&c.callee.as_str())
        })
    {
        let depth = b
            .calls
            .iter()
            .filter(|c| {
                SORT_NAMES.contains(&c.name.as_str()) || SORT_NAMES.contains(&c.callee.as_str())
            })
            .map(|c| c.loop_depth)
            .max()
            .unwrap_or(0);
        exp = exp.max(depth + 1);
    }
    on_stack[f] = false;
    memo[f] = Some((exp, exponential));
    (exp, exponential)
}

fn describe(
    ctx: &Ctx,
    f: usize,
    out: &[Vec<CallInfo>],
    exp: u32,
    exponential: bool,
    memo: &[Option<(u32, bool)>],
) -> (Complexity, bool) {
    let sym = ctx.symbol(f);
    let mut basis = Vec::new();
    let Some(body) = &sym.body else {
        return (
            Complexity {
                time: "unknown".into(),
                space: "unknown".into(),
                basis: vec!["no body".into()],
            },
            false,
        );
    };
    if body.max_loop_depth == 0 {
        basis.push("no loops".to_owned());
    } else {
        basis.push(format!("loop depth {}", body.max_loop_depth));
    }
    let mut noted: HashSet<usize> = HashSet::new();
    for call in &out[f] {
        if call.target == f {
            continue;
        }
        if call.loop_depth > 0 && call.trusted && !call.per_item && noted.insert(call.target) {
            let (k, _) = memo[call.target].unwrap_or((0, false));
            if k > 0 {
                basis.push(format!(
                    "calls {} ({}) inside a loop",
                    ctx.symbol(call.target).name,
                    poly(k, false)
                ));
            }
        }
    }
    let self_calls = &ctx.self_calls[f];
    let recursive = !self_calls.is_empty();
    if branching(self_calls) {
        basis.push(format!(
            "branching recursion ({} self-calls)",
            self_calls.len()
        ));
    } else if recursive {
        basis.push("self-recursive".to_owned());
    }
    let sort_depth = body
        .calls
        .iter()
        .filter(|c| {
            SORT_NAMES.contains(&c.name.as_str()) || SORT_NAMES.contains(&c.callee.as_str())
        })
        .map(|c| c.loop_depth + 1)
        .max();
    let log = sort_depth.is_some_and(|d| d >= exp);
    if sort_depth.is_some() {
        basis.push("sorts".to_owned());
    }
    let node = &ctx.nodes[ctx.fns[f].node];
    let io_in_loop: Vec<&str> = node
        .effects
        .direct
        .iter()
        .filter(|e| {
            e.loop_depth > 0
                && matches!(
                    e.kind,
                    EffectKind::Network
                        | EffectKind::DbRead
                        | EffectKind::DbWrite
                        | EffectKind::DbDelete
                        | EffectKind::FsRead
                        | EffectKind::FsWrite
                )
        })
        .map(|e| e.via.as_str())
        .collect();
    if !io_in_loop.is_empty() {
        let mut vias: Vec<&str> = io_in_loop;
        vias.sort_unstable();
        vias.dedup();
        basis.push(format!("I/O inside a loop ({})", vias.join(", ")));
    }
    let alloc_in_loop = body.calls.iter().any(|c| {
        c.loop_depth > 0
            && (ALLOC_NAMES.contains(&c.name.as_str()) || ALLOC_NAMES.contains(&c.callee.as_str()))
    });
    let space = if alloc_in_loop {
        basis.push("allocates inside a loop".to_owned());
        "O(n)"
    } else if recursive {
        basis.push("recursion depth on the stack".to_owned());
        "O(n)"
    } else {
        "O(1)"
    };
    let time = if exponential {
        "O(2^n)".to_owned()
    } else {
        poly(exp, log)
    };
    (
        Complexity {
            time,
            space: space.to_owned(),
            basis,
        },
        recursive,
    )
}

fn poly(exp: u32, log: bool) -> String {
    match (exp, log) {
        (0, false) => "O(1)".to_owned(),
        (0, true) => "O(log n)".to_owned(),
        (1, false) => "O(n)".to_owned(),
        (1, true) => "O(n log n)".to_owned(),
        (k, false) => format!("O(n^{k})"),
        (k, true) => format!("O(n^{k} log n)"),
    }
}

pub(crate) fn transitive(ctx: &mut Ctx) {
    let n = ctx.nodes.len();
    let mut mask: Vec<u16> = vec![0; n];
    for (i, node) in ctx.nodes.iter().enumerate() {
        for e in &node.effects.direct {
            mask[i] |= effect_bit(e.kind);
        }
    }
    let is_async: Vec<bool> = ctx
        .nodes
        .iter()
        .map(|nd| nd.function.as_ref().is_some_and(|f| f.is_async))
        .collect();
    // A sync helper that sleeps blocks a Rust/Python/Go async caller; JavaScript timers do not.
    let can_block: Vec<bool> = ctx
        .nodes
        .iter()
        .map(|nd| {
            !matches!(
                nd.language,
                Some(Language::TypeScript | Language::JavaScript)
            )
        })
        .collect();
    let blocking_sources = effect_bit(EffectKind::Sleep) | effect_bit(EffectKind::Blocking);
    // Concurrency facts of a server handler do not describe the client that requests it.
    let local_only = blocking_sources
        | effect_bit(EffectKind::Lock)
        | effect_bit(EffectKind::Spawn)
        | effect_bit(EffectKind::Join);
    let mut callers: Vec<Vec<(usize, EdgeKind, bool)>> = vec![Vec::new(); n];
    for (e, &(from, to)) in ctx.edges.iter().zip(&ctx.edge_ends) {
        let trusted = if matches!(e.kind, EdgeKind::Calls | EdgeKind::Spawns) {
            plausible_call(e)
        } else {
            !e.attrs.contains_key("candidates")
        };
        if FLOW_KINDS.contains(&e.kind) && trusted {
            let detached = e.kind == EdgeKind::Spawns || e.attrs.contains_key("ref");
            callers[to].push((from, e.kind, detached));
        }
    }
    let mut queue: VecDeque<usize> = (0..n).filter(|&i| mask[i] != 0).collect();
    let mut queued = vec![false; n];
    for &i in &queue {
        queued[i] = true;
    }
    while let Some(i) = queue.pop_front() {
        queued[i] = false;
        for &(c, kind, detached) in &callers[i] {
            let inherited = if kind == EdgeKind::Requests {
                mask[i] & !local_only
            } else if detached {
                mask[i] & !blocking_sources
            } else {
                mask[i]
            };
            let mut merged = mask[c] | inherited;
            if kind == EdgeKind::Calls
                && !detached
                && !is_async[i]
                && is_async[c]
                && can_block[i]
                && mask[i] & blocking_sources != 0
            {
                merged |= effect_bit(EffectKind::Blocking);
            }
            if merged != mask[c] {
                mask[c] = merged;
                if !queued[c] {
                    queued[c] = true;
                    queue.push_back(c);
                }
            }
        }
    }
    for (i, node) in ctx.nodes.iter_mut().enumerate() {
        node.effects.transitive = ALL_EFFECTS
            .iter()
            .copied()
            .filter(|k| mask[i] & effect_bit(*k) != 0)
            .collect();
    }
}

const HOOK_NAMES: &[&str] = &[
    "fmt",
    "drop",
    "clone",
    "eq",
    "ne",
    "cmp",
    "partial_cmp",
    "hash",
    "default",
    "from",
    "into",
    "try_from",
    "try_into",
    "deref",
    "deref_mut",
    "as_ref",
    "borrow",
    "serialize",
    "deserialize",
    "from_str",
    "index",
    "index_mut",
    "poll",
    "call",
    "new",
    "constructor",
    "toString",
    "toJSON",
    "valueOf",
    "render",
    "componentDidMount",
    "ngOnInit",
    "String",
    "Error",
    "ServeHTTP",
    "MarshalJSON",
    "UnmarshalJSON",
    "Read",
    "Write",
    "Close",
    "Less",
    "Swap",
    "Len",
    "init",
    "setUp",
    "tearDown",
    "setup",
    "teardown",
    "validate",
    "visit",
    "accept",
];

pub(crate) fn dead(ctx: &mut Ctx, root_dir: &std::path::Path) {
    let n = ctx.nodes.len();
    let mut next: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (e, &(from, to)) in ctx.edges.iter().zip(&ctx.edge_ends) {
        if FLOW_KINDS.contains(&e.kind) {
            next[from].push(to);
        }
    }
    let mut owned: HashMap<(usize, &str), Vec<usize>> = HashMap::new();
    let mut by_file: HashMap<usize, Vec<usize>> = HashMap::new();
    for f in 0..ctx.fns.len() {
        if let Some(p) = &ctx.symbol(f).parent
            && ctx.fns[f].parent_type.is_none()
        {
            owned
                .entry((ctx.fns[f].file, p.as_str()))
                .or_default()
                .push(f);
        }
        by_file.entry(ctx.fns[f].file).or_default().push(f);
    }
    let mut member_refs: Vec<Vec<String>> = vec![Vec::new(); ctx.fns.len()];
    for f in 0..ctx.fns.len() {
        let from = ctx.fns[f].node;
        let fi = ctx.fns[f].file;
        let sym = ctx.symbol(f);
        if let Some(p) = &sym.parent
            && let Some(&pf) = ctx.fn_by_file_local[fi].get(p)
        {
            next[ctx.fns[pf].node].push(from);
        }
        let script =
            super::resolve::family(ctx.files[fi].language) == super::resolve::Family::Script;
        let bound = |name: &str| ctx.scopes[fi].bindings.contains_key(name);
        let mut paths = value_refs(sym, script);
        let mut heads: Vec<String> = Vec::new();
        for receiver in sym
            .body
            .iter()
            .flat_map(|b| &b.calls)
            .filter_map(|c| c.receiver.as_deref())
        {
            let mut segs = receiver.split('.');
            heads.extend(segs.next().map(str::to_owned));
            member_refs[f].extend(segs.map(str::to_owned));
        }
        for path in &paths {
            if let Some((head, name)) = path.rsplit_once('.') {
                let first = head.split('.').next().unwrap_or(head);
                if !bound(first) {
                    member_refs[f].push(name.to_owned());
                }
                heads.push(first.to_owned());
            }
        }
        heads.retain(|h| !is_local(sym, h) && !matches!(h.as_str(), "self" | "this" | "cls"));
        heads.sort();
        heads.dedup();
        let mut objects: Vec<usize> = Vec::new();
        for path in paths.iter().filter(|p| !p.contains(['.', ':'])) {
            objects.extend(owned.get(&(fi, path.as_str())).into_iter().flatten());
            if let Some(&imp) = ctx.scopes[fi].bindings.get(path)
                && let Some(tf) = ctx.scopes[fi].imports[imp].file
            {
                let orig = ctx.scopes[fi].renames.get(path).unwrap_or(path);
                objects.extend(owned.get(&(tf, orig.as_str())).into_iter().flatten());
                let namespace = ctx.files[fi]
                    .imports
                    .iter()
                    .any(|i| i.alias.as_deref() == Some(path) && i.names.is_empty());
                if namespace {
                    objects.extend(
                        by_file
                            .get(&tf)
                            .into_iter()
                            .flatten()
                            .copied()
                            .filter(|&t| ctx.symbol(t).visibility == Visibility::Public),
                    );
                }
            }
        }
        for &t in &objects {
            next[from].push(ctx.fns[t].node);
        }
        paths.extend(heads);
        for path in paths {
            for t in super::resolve::resolve_ref(ctx, f, &path) {
                if t != f {
                    next[from].push(ctx.fns[t].node);
                }
            }
        }
    }

    let mut implementing_types: HashSet<usize> = HashSet::new();
    for (e, &(from, _)) in ctx.edges.iter().zip(&ctx.edge_ends) {
        if e.kind == EdgeKind::Implements {
            implementing_types.insert(from);
        }
    }
    for (fi, file) in ctx.files.iter().enumerate() {
        for sym in &file.symbols {
            if !sym.implements.is_empty()
                && let Some(&t) = ctx.type_by_file_local[fi].get(&sym.local_name)
            {
                implementing_types.insert(t);
            }
        }
    }
    let attribute_text: Vec<String> = ctx
        .files
        .iter()
        .map(|file| {
            file.symbols
                .iter()
                .flat_map(|s| s.attributes.iter())
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join("\n")
        })
        .collect();

    let mut exempt = vec![false; ctx.fns.len()];
    let mut root = vec![false; ctx.fns.len()];
    for f in 0..ctx.fns.len() {
        let sym = ctx.symbol(f);
        let fi = ctx.fns[f].file;
        let file = &ctx.files[fi];
        let nested = sym
            .parent
            .as_ref()
            .is_some_and(|p| ctx.fn_by_file_local[fi].contains_key(p));
        let called_externally = referenced_in_attributes(&attribute_text[fi], &sym.name)
            || !sym.implements.is_empty()
            || ctx.fns[f]
                .parent_type
                .is_some_and(|t| implementing_types.contains(&t))
            || HOOK_NAMES.contains(&sym.name.as_str())
            || sym.name.starts_with("__")
            || (file.language == Language::Go
                && sym.visibility == Visibility::Public
                && !file.path.starts_with("cmd/")
                && file.module != "main")
            || sym.attributes.iter().any(|a| {
                a.contains("no_mangle") || a.contains("export") || a.contains("wasm_bindgen")
            });
        root[f] = called_externally || framework_root(file, sym);
        exempt[f] = sym.body.is_none() || nested || called_externally;
    }
    for f in library_api(ctx, root_dir)
        .into_iter()
        .chain(files_named_by_path(ctx))
        .chain(super::packages::python_entry_points(ctx, root_dir))
    {
        root[f] = true;
    }

    let mut reachable = vec![false; n];
    let mut stack: Vec<usize> = ctx
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, nd)| nd.entrypoint.is_some())
        .map(|(i, _)| i)
        .chain(
            (0..ctx.fns.len())
                .filter(|&f| root[f])
                .map(|f| ctx.fns[f].node),
        )
        .collect();
    for &s in &stack {
        reachable[s] = true;
    }
    let mut live_method_names: HashSet<(super::resolve::Family, &str)> = HashSet::new();
    let mut scanned = vec![false; ctx.fns.len()];
    loop {
        while let Some(i) = stack.pop() {
            for &t in &next[i] {
                if !reachable[t] {
                    reachable[t] = true;
                    stack.push(t);
                }
            }
        }
        // Methods called or read on receivers of unknown type are kept alive by name, and
        // become roots.
        for f in 0..ctx.fns.len() {
            if scanned[f] || !reachable[ctx.fns[f].node] {
                continue;
            }
            scanned[f] = true;
            let family = super::resolve::family(ctx.files[ctx.fns[f].file].language);
            for call in ctx.symbol(f).body.iter().flat_map(|b| &b.calls) {
                if call.receiver.is_some() {
                    live_method_names.insert((family, call.name.as_str()));
                }
            }
            for name in &member_refs[f] {
                live_method_names.insert((family, name.as_str()));
            }
        }
        for f in 0..ctx.fns.len() {
            let node = ctx.fns[f].node;
            let sym = ctx.symbol(f);
            if !reachable[node]
                && sym.kind == SymbolKind::Method
                && live_method_names.contains(&(
                    super::resolve::family(ctx.files[ctx.fns[f].file].language),
                    sym.name.as_str(),
                ))
            {
                reachable[node] = true;
                stack.push(node);
            }
        }
        if stack.is_empty() {
            break;
        }
    }

    for f in 0..ctx.fns.len() {
        let node = ctx.fns[f].node;
        if !reachable[node] && !exempt[f] {
            ctx.nodes[node].dead = true;
        }
    }
}

/// Test doubles that replace a module named by path rather than run it.
const MOCK_CALLS: &[&str] = &["mock", "doMock", "unmock", "importActual", "requireActual"];

/// Top-level code and public functions of script files that code names by relative path
/// (`new URL('./src/testing/paths.ts', import.meta.url)`, `new Worker('./worker.js')`).
fn files_named_by_path(ctx: &Ctx) -> Vec<usize> {
    let mut files: HashSet<usize> = HashSet::new();
    for f in 0..ctx.fns.len() {
        let fi = ctx.fns[f].file;
        if super::resolve::family(ctx.files[fi].language) != super::resolve::Family::Script {
            continue;
        }
        for arg in ctx
            .symbol(f)
            .body
            .iter()
            .flat_map(|b| &b.calls)
            .filter(|c| !MOCK_CALLS.contains(&c.name.as_str()))
            .flat_map(|c| &c.string_args)
        {
            let relative = arg.starts_with("./") || arg.starts_with("../");
            let source = [
                ".ts", ".js", ".mjs", ".cjs", ".mts", ".tsx", ".jsx", ".svelte",
            ]
            .iter()
            .any(|ext| arg.ends_with(ext));
            if relative && source {
                let base = arg.rsplit('/').next().unwrap_or(arg);
                files.extend(
                    super::resolve::resolve_import(ctx, fi, arg)
                        .into_iter()
                        .filter(|&tf| ctx.files[tf].path.rsplit('/').next() == Some(base)),
                );
            }
        }
    }
    (0..ctx.fns.len())
        .filter(|&f| {
            let sym = ctx.symbol(f);
            files.contains(&ctx.fns[f].file)
                && (sym.visibility == Visibility::Public || sym.name == "<module>")
        })
        .collect()
}

/// Public functions of a published package: those its manifest entry files define, and those
/// they re-export (`export * from`, `export { a } from`), transitively.
fn library_api(ctx: &Ctx, root: &std::path::Path) -> Vec<usize> {
    let mut by_file: HashMap<usize, Vec<usize>> = HashMap::new();
    for (f, rec) in ctx.fns.iter().enumerate() {
        by_file.entry(rec.file).or_default().push(f);
    }
    let mut seen: HashSet<(usize, String)> = HashSet::new();
    let mut pending: Vec<(usize, String)> = super::packages::library_entries(ctx, root)
        .into_iter()
        .map(|fi| (fi, "*".to_owned()))
        .collect();
    let mut out = Vec::new();
    while let Some((fi, name)) = pending.pop() {
        if !seen.insert((fi, name.clone())) {
            continue;
        }
        let public: Vec<usize> = by_file
            .get(&fi)
            .into_iter()
            .flatten()
            .copied()
            .filter(|&f| ctx.symbol(f).visibility == Visibility::Public)
            .collect();
        let owned = |f: usize| {
            let sym = ctx.symbol(f);
            name == "*" || sym.parent.as_deref().unwrap_or(&sym.name) == name
        };
        if public.iter().any(|&f| owned(f)) {
            out.extend(public.iter().copied().filter(|&f| owned(f)));
        } else {
            // A re-exported value that is not a function (`theme = new Theme()`) exposes the
            // methods of the file's types.
            out.extend(
                public
                    .iter()
                    .copied()
                    .filter(|&f| ctx.symbol(f).kind == SymbolKind::Method),
            );
        }
        for imp in ctx.files[fi].imports.iter().filter(|i| i.reexport) {
            let names: Vec<String> = imp
                .names
                .iter()
                .map(|n| n.split(" as ").next().unwrap_or(n).trim().to_owned())
                .filter(|n| name == "*" || *n == name || n == "*")
                .collect();
            for tf in super::resolve::resolve_import(ctx, fi, &imp.path) {
                pending.extend(names.iter().map(|n| (tf, n.clone())));
            }
        }
    }
    out
}

/// A bare name the function binds itself: a parameter or a local assignment.
fn is_local(sym: &hoshizora_model::Symbol, name: &str) -> bool {
    !name.contains(['.', ':'])
        && sym.name != "<module>"
        && (sym.params.iter().any(|p| p.name == name)
            || sym
                .body
                .iter()
                .flat_map(|b| &b.calls)
                .any(|c| c.binds.iter().any(|b| b == name)))
}

/// Identifier paths in `text`: `defaultStorage()` → `defaultStorage`, `a.b(c)` → `a.b`, `c`.
fn paths_in(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !(c.is_alphanumeric() || matches!(c, '_' | '$' | '.')))
        .map(|w| w.trim_matches('.'))
        .filter(|w| !w.is_empty() && !w.starts_with(|c: char| c.is_ascii_digit()))
}

/// References a body makes to functions as values, plus those in parameter defaults
/// (`run = run_command`, `storage = defaultStorage()`, `{ sleep = defaultSleep } = {}`);
/// names the function binds itself are left out. In scripts, `$store` reads `store`.
fn value_refs(sym: &hoshizora_model::Symbol, script: bool) -> Vec<String> {
    let Some(body) = &sym.body else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    for p in &sym.params {
        out.extend(
            p.default
                .as_deref()
                .into_iter()
                .flat_map(paths_in)
                .map(str::to_owned),
        );
        for part in p.name.split([',', '{', '}', '[', ']']) {
            if let Some((_, value)) = part.split_once('=') {
                out.extend(paths_in(value).map(str::to_owned));
            }
        }
    }
    for r in &body.refs {
        let r = match r.strip_prefix('$') {
            Some(store) if script && !store.is_empty() => store,
            _ => r.as_str(),
        };
        if !is_local(sym, r) {
            out.push(r.to_owned());
        }
    }
    out.sort();
    out.dedup();
    out
}

const SVELTEKIT_ROUTE_EXPORTS: &[&str] = &["load", "entries"];
const SVELTEKIT_ENDPOINTS: &[&str] = &[
    "GET", "POST", "PUT", "PATCH", "DELETE", "OPTIONS", "HEAD", "fallback",
];
const SVELTEKIT_HOOKS: &[&str] = &[
    "handle",
    "handleFetch",
    "handleError",
    "handleValidationError",
    "reroute",
    "init",
    "transport",
];
/// Decorator namespaces that wrap a function rather than register it somewhere.
const WRAPPING_DECORATORS: &[&str] = &[
    "functools",
    "contextlib",
    "abc",
    "typing",
    "typing_extensions",
    "dataclasses",
    "pytest",
    "unittest",
    "mock",
    "enum",
    "warnings",
    "backoff",
    "tenacity",
    "torch",
    "numba",
    "jax",
    "np",
    "numpy",
];
const PROPERTY_DECORATORS: &[&str] = &["property", "cached_property", "functools.cached_property"];
/// Rust attributes that export a function to a host runtime.
const FFI_ATTRIBUTES: &[&str] = &[
    "pyfunction",
    "pymethods",
    "pymodule",
    "pyo3::pymodule",
    "pyo3::pyfunction",
    "pyo3::pymethods",
    "napi",
    "tauri::command",
    "uniffi::export",
    "export_name",
];

/// Functions a framework or runtime calls by convention: SvelteKit route modules, endpoints
/// and hooks, functions registered through a decorator on an object (`@app.command()`,
/// `@router.get("/")`), properties, methods of a default-exported object, FFI exports.
fn framework_root(file: &hoshizora_model::FileFacts, sym: &hoshizora_model::Symbol) -> bool {
    let base = file.path.rsplit('/').next().unwrap_or(&file.path);
    let stem = base.split('.').next().unwrap_or(base);
    let script = matches!(file.language, Language::TypeScript | Language::JavaScript)
        && !base.ends_with(".svelte");
    if script {
        let top = sym.parent.is_none();
        let route = matches!(stem, "+page" | "+layout");
        if route
            && ((top && SVELTEKIT_ROUTE_EXPORTS.contains(&sym.name.as_str()))
                || sym.parent.as_deref() == Some("actions"))
        {
            return true;
        }
        if stem == "+server" && top && SVELTEKIT_ENDPOINTS.contains(&sym.name.as_str()) {
            return true;
        }
        if stem == "hooks" && top && SVELTEKIT_HOOKS.contains(&sym.name.as_str()) {
            return true;
        }
        if sym.parent.as_deref() == Some("default") {
            return true;
        }
    }
    sym.attributes.iter().any(|a| {
        let head = a.split('(').next().unwrap_or(a).trim();
        match file.language {
            Language::Python => {
                PROPERTY_DECORATORS.contains(&head)
                    || head
                        .split_once('.')
                        .is_some_and(|(ns, _)| !WRAPPING_DECORATORS.contains(&ns))
            }
            Language::Rust => FFI_ATTRIBUTES.contains(&head),
            _ => false,
        }
    })
}

/// `#[serde(default = "default_site")]`, `@traced`, `@app.on_event(startup_hook)`.
fn referenced_in_attributes(attributes: &str, name: &str) -> bool {
    attributes
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .any(|word| word == name)
}

/// Two self-calls on one path (`f(n - 1) + f(n - 2)`, or two unconditional calls) branch;
/// one call per match arm or per child does not.
/// Two or more unconditional self-calls, or two on one line, that do not each descend into a
/// part of the input (a tree walk visits every node once).
fn branching(self_calls: &[super::SelfCall]) -> bool {
    let repeats: Vec<&super::SelfCall> = self_calls.iter().filter(|c| !c.on_part).collect();
    let unconditional = repeats.iter().filter(|c| !c.conditional).count();
    let same_line = repeats
        .iter()
        .enumerate()
        .any(|(i, c)| repeats[i + 1..].iter().any(|d| d.line == c.line));
    unconditional >= 2 || same_line
}
