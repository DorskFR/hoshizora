//! Entrypoints, complexity estimates, transitive effects and dead-code marking.

use std::collections::{HashMap, HashSet, VecDeque};

use hoshizora_model::{
    Complexity, EdgeKind, EffectKind, EntrypointKind, Language, SymbolKind, Visibility,
};

use super::Ctx;

/// Edge kinds along which control and effects flow.
pub const FLOW_KINDS: &[EdgeKind] = &[
    EdgeKind::Calls,
    EdgeKind::Spawns,
    EdgeKind::Handles,
    EdgeKind::Requests,
];

pub const ALL_EFFECTS: [EffectKind; 13] = [
    EffectKind::Network,
    EffectKind::DbRead,
    EffectKind::DbWrite,
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

fn is_test_file(path: &str) -> bool {
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
/// recursion. Cycles other than direct self-calls contribute nothing (treated as unknown).
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
        if call.target == f {
            continue;
        }
        let (k, e) = exponent(ctx, call.target, out, memo, on_stack);
        exponential |= e;
        exp = exp.max(k + call.loop_depth);
    }
    match ctx.self_calls[f] {
        0 => {}
        1 => exp = exp.max(1),
        _ => exponential = true,
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
        if call.loop_depth > 0 && noted.insert(call.target) {
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
    let recursive = ctx.self_calls[f] > 0;
    match ctx.self_calls[f] {
        0 => {}
        1 => basis.push("self-recursive".to_owned()),
        n => basis.push(format!("branching recursion ({n} self-calls)")),
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
    let mut callers: Vec<Vec<(usize, EdgeKind)>> = vec![Vec::new(); n];
    for (e, &(from, to)) in ctx.edges.iter().zip(&ctx.edge_ends) {
        if FLOW_KINDS.contains(&e.kind) && !e.attrs.contains_key("candidates") {
            callers[to].push((from, e.kind));
        }
    }
    let mut queue: VecDeque<usize> = (0..n).filter(|&i| mask[i] != 0).collect();
    let mut queued = vec![false; n];
    for &i in &queue {
        queued[i] = true;
    }
    while let Some(i) = queue.pop_front() {
        queued[i] = false;
        for &(c, kind) in &callers[i] {
            let inherited = if kind == EdgeKind::Requests {
                mask[i] & !local_only
            } else {
                mask[i]
            };
            let mut merged = mask[c] | inherited;
            if kind == EdgeKind::Calls
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

pub(crate) fn dead(ctx: &mut Ctx) {
    let n = ctx.nodes.len();
    let mut next: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (e, &(from, to)) in ctx.edges.iter().zip(&ctx.edge_ends) {
        if FLOW_KINDS.contains(&e.kind) {
            next[from].push(to);
        }
    }
    let mut reachable = vec![false; n];
    let mut stack: Vec<usize> = ctx
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, nd)| nd.entrypoint.is_some())
        .map(|(i, _)| i)
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
        // Methods called on receivers of unknown type are kept alive by name, and become roots.
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

    for f in 0..ctx.fns.len() {
        let node = ctx.fns[f].node;
        if reachable[node] {
            continue;
        }
        let sym = ctx.symbol(f);
        let fi = ctx.fns[f].file;
        let file = &ctx.files[fi];
        let nested = sym
            .parent
            .as_ref()
            .is_some_and(|p| ctx.fn_by_file_local[fi].contains_key(p));
        let exempt = sym.body.is_none()
            || nested
            || referenced_in_attributes(&attribute_text[fi], &sym.name)
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
        if !exempt {
            ctx.nodes[node].dead = true;
        }
    }
}

/// `#[serde(default = "default_site")]`, `@traced`, `@app.on_event(startup_hook)`.
fn referenced_in_attributes(attributes: &str, name: &str) -> bool {
    attributes
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .any(|word| word == name)
}
