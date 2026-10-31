use std::path::Path;

use hoshizora_core::{Index, Packs, build_graph};
use hoshizora_model::{
    Body, CallSite, EdgeKind, EffectKind, EntrypointKind, FileFacts, Import, Insight, Language,
    NodeKind, Span, Symbol, SymbolKind, Visibility,
};

fn call(callee: &str, line: u32) -> CallSite {
    let name = callee
        .rsplit(['.', ':'])
        .next()
        .unwrap_or(callee)
        .to_owned();
    CallSite {
        callee: callee.into(),
        name,
        receiver: None,
        string_args: vec![],
        line,
        loop_depth: 0,
        awaited: false,
        conditional: false,
        in_closure: false,
    }
}

fn args(mut c: CallSite, a: &[&str]) -> CallSite {
    c.string_args = a.iter().map(|s| (*s).to_owned()).collect();
    c
}

fn awaited(mut c: CallSite) -> CallSite {
    c.awaited = true;
    c
}

fn in_loop(mut c: CallSite, depth: u32) -> CallSite {
    c.loop_depth = depth;
    c
}

fn in_closure(mut c: CallSite) -> CallSite {
    c.in_closure = true;
    c
}

fn import(path: &str, names: &[&str]) -> Import {
    Import {
        path: path.into(),
        alias: None,
        names: names.iter().map(|s| (*s).to_owned()).collect(),
        line: 1,
    }
}

fn func(local: &str, start: u32, end: u32, calls: Vec<CallSite>) -> Symbol {
    let (parent, name) = match local.rsplit_once("::").or_else(|| local.rsplit_once('.')) {
        Some((p, n)) => (Some(p.to_owned()), n.to_owned()),
        None => (None, local.to_owned()),
    };
    let loops = calls.iter().map(|c| c.loop_depth).max().unwrap_or(0);
    Symbol {
        kind: if parent.is_some() {
            SymbolKind::Method
        } else {
            SymbolKind::Function
        },
        name,
        local_name: local.into(),
        parent,
        signature: format!("fn {local}()"),
        params: vec![],
        returns: None,
        doc: None,
        span: Span {
            start_line: start,
            end_line: end,
        },
        is_async: false,
        visibility: Visibility::Private,
        attributes: vec![],
        implements: vec![],
        body: Some(Body {
            calls,
            loops,
            max_loop_depth: loops,
            cyclomatic: 1,
            ..Default::default()
        }),
    }
}

fn ty(local: &str, start: u32, implements: &[&str]) -> Symbol {
    Symbol {
        kind: SymbolKind::Type,
        name: local.into(),
        local_name: local.into(),
        parent: None,
        signature: format!("struct {local}"),
        params: vec![],
        returns: None,
        doc: None,
        span: Span {
            start_line: start,
            end_line: start + 2,
        },
        is_async: false,
        visibility: Visibility::Public,
        attributes: vec![],
        implements: implements.iter().map(|s| (*s).to_owned()).collect(),
        body: None,
    }
}

fn file(
    path: &str,
    language: Language,
    module: &str,
    imports: Vec<Import>,
    symbols: Vec<Symbol>,
) -> FileFacts {
    FileFacts {
        path: path.into(),
        language,
        module: module.into(),
        lines: 200,
        imports,
        symbols,
    }
}

fn fixture() -> Vec<FileFacts> {
    let mut main = func(
        "main",
        5,
        25,
        vec![
            call("Router::new", 10),
            args(call("Router::new().route", 11), &["/api/sessions"]),
            call("get", 11),
            call("list_sessions", 11),
            call("post", 11),
            call("create_session", 11),
            call("tokio::spawn", 20),
            in_closure(awaited(call("background_job", 20))),
            call("axum::serve", 24),
        ],
    );
    main.attributes = vec!["tokio::main".into()];
    main.is_async = true;

    let mut list_sessions = func(
        "list_sessions",
        30,
        40,
        vec![
            args(
                call("sqlx::query_as", 31),
                &["SELECT id, name FROM sessions WHERE active = $1"],
            ),
            awaited(call("sqlx::query_as(..).fetch_all", 32)),
            CallSite {
                receiver: Some("store".into()),
                ..call("store.list", 35)
            },
        ],
    );
    list_sessions.is_async = true;
    list_sessions.params = vec![hoshizora_model::Param {
        name: "store".into(),
        ty: Some("State<SessionStore>".into()),
        default: None,
    }];

    let mut create_session = func(
        "create_session",
        42,
        50,
        vec![
            args(
                call("sqlx::query", 43),
                &["INSERT INTO sessions (id, name) VALUES ($1, $2)"],
            ),
            awaited(call("sqlx::query(..).execute", 44)),
        ],
    );
    create_session.is_async = true;

    let mut background_job = func(
        "background_job",
        52,
        60,
        vec![
            in_loop(
                args(
                    call("sqlx::query", 54),
                    &["SELECT * FROM events WHERE id = $1"],
                ),
                1,
            ),
            in_loop(awaited(call("sqlx::query(..).fetch_one", 55)), 1),
            in_loop(call("results.push", 56), 1),
        ],
    );
    background_job.is_async = true;

    let unused_helper = func("unused_helper", 62, 64, vec![]);
    let ambiguous = func(
        "ambiguous",
        66,
        70,
        vec![call("helper", 67), call("unused_helper", 68)],
    );
    let mut fmt = func("Formatter::fmt", 80, 84, vec![]);
    fmt.kind = SymbolKind::Method;
    let draw = func("Widget::draw", 90, 94, vec![]);

    let api_lib = file(
        "crates/api/src/lib.rs",
        Language::Rust,
        "crate",
        vec![
            import("sqlx::PgPool", &[]),
            import("axum::Router", &[]),
            import("axum::routing::get", &[]),
            import("crate::store::SessionStore", &[]),
        ],
        vec![
            main,
            list_sessions,
            create_session,
            background_job,
            unused_helper,
            ambiguous,
            ty("Formatter", 78, &["std::fmt::Display"]),
            fmt,
            ty("Widget", 88, &["Render"]),
            draw,
        ],
    );

    let mut store_list = func(
        "SessionStore::list",
        10,
        20,
        vec![
            args(call("sqlx::query_as", 12), &["SELECT id FROM sessions"]),
            awaited(call("sqlx::query_as(..).fetch_all", 13)),
            awaited(call("sqlx::query_as(..).fetch_all", 14)),
        ],
    );
    store_list.is_async = true;
    let api_store = file(
        "crates/api/src/store.rs",
        Language::Rust,
        "crate::store",
        vec![import("sqlx::PgPool", &[])],
        vec![ty("SessionStore", 5, &[]), store_list],
    );

    let mut fetch_sessions = func(
        "fetchSessions",
        3,
        8,
        vec![awaited(args(call("fetch", 4), &["/api/sessions"]))],
    );
    fetch_sessions.is_async = true;
    fetch_sessions.visibility = Visibility::Public;
    let mut create_session_ts = func(
        "createSession",
        10,
        14,
        vec![awaited(args(call("axios.post", 11), &["/api/sessions"]))],
    );
    create_session_ts.is_async = true;
    create_session_ts.visibility = Visibility::Public;
    let mut github = func(
        "fetchStars",
        16,
        20,
        vec![awaited(args(
            call("fetch", 17),
            &["https://api.github.com/repos/x/y"],
        ))],
    );
    github.is_async = true;
    let webui_api = file(
        "webui/src/api.ts",
        Language::TypeScript,
        "src/api",
        vec![import("axios", &[])],
        vec![fetch_sessions, create_session_ts, github],
    );

    let mut handle_click = func(
        "handleClick",
        5,
        12,
        vec![
            awaited(call("createSession", 6)),
            awaited(call("fetchSessions", 7)),
            awaited(call("fetchStars", 8)),
        ],
    );
    handle_click.is_async = true;
    let app = file(
        "webui/src/App.svelte",
        Language::TypeScript,
        "src/App",
        vec![import(
            "./api",
            &["createSession", "fetchSessions", "fetchStars"],
        )],
        vec![handle_click],
    );

    let mut files = vec![api_lib, api_store, webui_api, app];
    for i in 0..4 {
        files.push(file(
            &format!("webui/src/h{i}.ts"),
            Language::TypeScript,
            &format!("src/h{i}"),
            vec![],
            vec![func("helper", 1, 3, vec![])],
        ));
    }

    let fib = func("fib", 1, 5, vec![call("fib", 3), call("fib", 3)]);
    let walk = func("walk", 7, 12, vec![in_loop(call("print", 9), 2)]);
    let process = func(
        "process",
        14,
        18,
        vec![in_loop(call("walk", 16), 1), call("sorted", 17)],
    );
    let mut blocking = func("slow", 20, 24, vec![call("time.sleep", 22)]);
    blocking.is_async = true;
    files.push(file(
        "app/jobs.py",
        Language::Python,
        "app.jobs",
        vec![import("time", &[])],
        vec![fib, walk, process, blocking],
    ));

    let go_main = func(
        "main",
        1,
        10,
        vec![
            call("go", 5),
            in_closure(call("worker", 5)),
            call("wg.Wait", 8),
        ],
    );
    let worker = func(
        "worker",
        12,
        20,
        vec![args(call("http.Get", 14), &["https://example.com/health"])],
    );
    files.push(file(
        "cmd/svc/main.go",
        Language::Go,
        "main",
        vec![import("net/http", &[]), import("sync", &[])],
        vec![go_main, worker],
    ));
    files
}

fn build() -> Index {
    Index::new(build_graph(
        Path::new("/repo"),
        fixture(),
        &Packs::builtin(),
    ))
}

fn edges_between<'a>(idx: &'a Index, from: &str, to: &str) -> Vec<&'a hoshizora_model::Edge> {
    idx.outgoing(from)
        .into_iter()
        .filter(|e| e.to == to)
        .collect()
}

#[test]
fn cross_file_resolution_via_import() {
    let idx = build();
    let edges = edges_between(
        &idx,
        "fn:webui/src/App.svelte#handleClick",
        "fn:webui/src/api.ts#createSession",
    );
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].kind, EdgeKind::Calls);
    assert_eq!(edges[0].confidence, hoshizora_model::Confidence::Exact);
    assert_eq!(edges[0].line, Some(6));
    assert!(edges[0].awaited);
    let imports = edges_between(&idx, "file:webui/src/App.svelte", "file:webui/src/api.ts");
    assert_eq!(imports.len(), 1);
    assert_eq!(imports[0].kind, EdgeKind::Imports);
    assert!(idx.node("ext:axios").is_some());
    assert!(!edges_between(&idx, "file:webui/src/api.ts", "ext:axios").is_empty());
    assert!(!edges_between(&idx, "fn:webui/src/api.ts#createSession", "ext:axios").is_empty());
}

#[test]
fn receiver_typed_resolution() {
    let idx = build();
    let edges = edges_between(
        &idx,
        "fn:crates/api/src/lib.rs#list_sessions",
        "fn:crates/api/src/store.rs#SessionStore::list",
    );
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].confidence, hoshizora_model::Confidence::Exact);
}

#[test]
fn ambiguous_names_are_dropped() {
    let idx = build();
    let out = idx.outgoing("fn:crates/api/src/lib.rs#ambiguous");
    assert!(out.iter().all(|e| !e.to.contains("#helper")), "{out:?}");
    assert!(
        out.iter()
            .any(|e| e.to == "fn:crates/api/src/lib.rs#unused_helper")
    );
    assert!(
        idx.graph()
            .meta
            .warnings
            .iter()
            .any(|w| w.starts_with("ambiguous calls dropped") && w.ends_with(": 1")),
        "{:?}",
        idx.graph().meta.warnings
    );
}

#[test]
fn sqlx_tables_read_and_write() {
    let idx = build();
    let list = idx.node("fn:crates/api/src/lib.rs#list_sessions").unwrap();
    let read = list
        .effects
        .direct
        .iter()
        .find(|e| e.kind == EffectKind::DbRead)
        .expect("db read");
    assert_eq!(read.target.as_deref(), Some("sessions"));
    assert_eq!(read.rule, "rust.sqlx.query");
    assert!(
        !edges_between(&idx, &list.id, "table:sessions")
            .iter()
            .any(|e| e.kind == EdgeKind::Writes)
    );
    assert!(
        edges_between(&idx, &list.id, "table:sessions")
            .iter()
            .any(|e| e.kind == EdgeKind::Reads)
    );

    let create = idx.node("fn:crates/api/src/lib.rs#create_session").unwrap();
    assert!(
        create
            .effects
            .direct
            .iter()
            .any(|e| e.kind == EffectKind::DbWrite && e.target.as_deref() == Some("sessions"))
    );
    assert!(
        edges_between(&idx, &create.id, "table:sessions")
            .iter()
            .any(|e| e.kind == EdgeKind::Writes)
    );
    assert_eq!(idx.node("table:sessions").unwrap().kind, NodeKind::Table);
    assert_eq!(
        idx.with_effect(EffectKind::DbWrite, Some("sessions")).len(),
        1
    );
}

#[test]
fn routes_link_clients_and_propagate_effects() {
    let idx = build();
    let get = idx.node("route:GET /api/sessions").expect("GET route");
    let post = idx.node("route:POST /api/sessions").expect("POST route");
    assert!(
        edges_between(&idx, &get.id, "fn:crates/api/src/lib.rs#list_sessions")
            .iter()
            .any(|e| e.kind == EdgeKind::Handles)
    );
    assert!(
        edges_between(&idx, &post.id, "fn:crates/api/src/lib.rs#create_session")
            .iter()
            .any(|e| e.kind == EdgeKind::Handles)
    );
    assert_eq!(
        idx.node("fn:crates/api/src/lib.rs#list_sessions")
            .unwrap()
            .entrypoint,
        Some(EntrypointKind::HttpHandler)
    );

    let create_ts = idx.node("fn:webui/src/api.ts#createSession").unwrap();
    let req: Vec<_> = idx
        .outgoing(&create_ts.id)
        .into_iter()
        .filter(|e| e.kind == EdgeKind::Requests)
        .collect();
    assert_eq!(req.len(), 1);
    assert_eq!(req[0].to, post.id);
    assert_eq!(req[0].confidence, hoshizora_model::Confidence::Heuristic);
    assert!(
        create_ts.effects.transitive.contains(&EffectKind::DbWrite),
        "{:?}",
        create_ts.effects.transitive
    );
    assert!(
        create_ts
            .effects
            .direct
            .iter()
            .any(|e| e.kind == EffectKind::Network)
    );

    let fetch_ts = idx.node("fn:webui/src/api.ts#fetchSessions").unwrap();
    let req: Vec<_> = idx
        .outgoing(&fetch_ts.id)
        .into_iter()
        .filter(|e| e.kind == EdgeKind::Requests)
        .map(|e| e.to.clone())
        .collect();
    assert!(req.contains(&get.id));
    assert!(fetch_ts.effects.transitive.contains(&EffectKind::DbRead));

    let click = idx.node("fn:webui/src/App.svelte#handleClick").unwrap();
    assert!(click.effects.transitive.contains(&EffectKind::DbWrite));
    assert_eq!(click.entrypoint, Some(EntrypointKind::UiHandler));

    let host = idx.node("host:api.github.com").expect("external host node");
    assert!(
        edges_between(&idx, "fn:webui/src/api.ts#fetchStars", &host.id)
            .iter()
            .any(|e| e.kind == EdgeKind::Requests)
    );
}

#[test]
fn spawn_edges() {
    let idx = build();
    let edges = edges_between(
        &idx,
        "fn:crates/api/src/lib.rs#main",
        "fn:crates/api/src/lib.rs#background_job",
    );
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].kind, EdgeKind::Spawns);
    let main = idx.node("fn:crates/api/src/lib.rs#main").unwrap();
    assert!(
        main.effects
            .direct
            .iter()
            .any(|e| e.kind == EffectKind::Spawn && e.rule == "rust.tokio.spawn")
    );
    assert_eq!(main.entrypoint, Some(EntrypointKind::Main));
    assert!(main.effects.transitive.contains(&EffectKind::DbRead));

    let go = edges_between(&idx, "fn:cmd/svc/main.go#main", "fn:cmd/svc/main.go#worker");
    assert_eq!(go.len(), 1);
    assert_eq!(go[0].kind, EdgeKind::Spawns);
    let go_main = idx.node("fn:cmd/svc/main.go#main").unwrap();
    assert!(
        go_main
            .effects
            .direct
            .iter()
            .any(|e| e.kind == EffectKind::Join)
    );
    assert!(go_main.effects.transitive.contains(&EffectKind::Network));
}

#[test]
fn dead_detection_with_trait_impl_exemption() {
    let idx = build();
    let dead: Vec<&str> = idx.dead().iter().map(|n| n.id.as_str()).collect();
    assert!(
        dead.contains(&"fn:crates/api/src/lib.rs#ambiguous"),
        "{dead:?}"
    );
    assert!(
        dead.contains(&"fn:crates/api/src/lib.rs#unused_helper"),
        "{dead:?}"
    );
    assert!(!dead.contains(&"fn:crates/api/src/lib.rs#Formatter::fmt"));
    assert!(!dead.contains(&"fn:crates/api/src/lib.rs#Widget::draw"));
    assert!(!dead.contains(&"fn:crates/api/src/lib.rs#list_sessions"));
    assert!(!dead.contains(&"fn:crates/api/src/lib.rs#background_job"));
    assert!(!dead.contains(&"fn:crates/api/src/store.rs#SessionStore::list"));
    assert!(dead.iter().all(|d| d.starts_with("fn:")));
}

#[test]
fn flow_and_impact() {
    let idx = build();
    let flow = idx
        .flow("fn:webui/src/App.svelte#handleClick", 6, &[])
        .unwrap();
    assert!(flow.nodes.contains(&"route:POST /api/sessions".to_owned()));
    assert!(
        flow.nodes
            .contains(&"fn:crates/api/src/lib.rs#create_session".to_owned())
    );
    assert!(flow.nodes.contains(&"table:sessions".to_owned()));
    assert!(!flow.truncated);
    assert_eq!(flow.steps[0].depth, 0);
    assert!(flow.steps.iter().any(|s| s.lane == "webui"));
    assert!(flow.steps.iter().any(|s| s.lane == "api"));
    assert!(flow.steps.iter().any(|s| s.lane == "tables"));
    let shallow = idx
        .flow("fn:webui/src/App.svelte#handleClick", 1, &[])
        .unwrap();
    assert!(!shallow.nodes.contains(&"table:sessions".to_owned()));

    let impact = idx.impact("table:sessions", 10).unwrap();
    assert!(
        impact
            .affected
            .contains(&"fn:webui/src/App.svelte#handleClick".to_owned())
    );
    let click = impact
        .entrypoints
        .iter()
        .find(|e| e.node == "fn:webui/src/App.svelte#handleClick")
        .expect("ui entrypoint");
    assert_eq!(click.kind, EntrypointKind::UiHandler);
    assert_eq!(
        click.path.first().map(String::as_str),
        Some("fn:webui/src/App.svelte#handleClick")
    );
    assert_eq!(
        click.path.last().map(String::as_str),
        Some("table:sessions")
    );
    assert!(click.path.len() >= 4);
    assert!(
        impact
            .entrypoints
            .iter()
            .any(|e| e.kind == EntrypointKind::Main)
    );
}

#[test]
fn complexity_estimates() {
    let idx = build();
    let f = |id: &str| idx.node(id).unwrap().function.clone().unwrap();
    let fib = f("fn:app/jobs.py#fib");
    assert_eq!(fib.complexity.time, "O(2^n)");
    assert!(fib.recursive);
    assert!(
        fib.complexity
            .basis
            .iter()
            .any(|b| b.contains("branching recursion"))
    );
    assert_eq!(f("fn:app/jobs.py#walk").complexity.time, "O(n^2)");
    let process = f("fn:app/jobs.py#process");
    assert_eq!(process.complexity.time, "O(n^3)");
    assert!(
        process
            .complexity
            .basis
            .iter()
            .any(|b| b.contains("calls walk (O(n^2)) inside a loop"))
    );
    assert_eq!(process.fan_out, 1);
    assert_eq!(f("fn:app/jobs.py#walk").fan_in, 1);
    let job = f("fn:crates/api/src/lib.rs#background_job");
    assert_eq!(job.complexity.time, "O(n)");
    assert_eq!(job.complexity.space, "O(n)");
    assert!(
        job.complexity
            .basis
            .iter()
            .any(|b| b.starts_with("I/O inside a loop"))
    );
    assert_eq!(
        f("fn:crates/api/src/lib.rs#unused_helper").complexity.time,
        "O(1)"
    );
}

#[test]
fn static_insights() {
    let idx = build();
    let insights = idx.static_insights();
    assert!(insights.iter().any(|i| matches!(i, Insight::IoInLoop { node, line, .. } if node == "fn:crates/api/src/lib.rs#background_job" && *line == 55)), "{insights:?}");
    assert!(insights.iter().any(|i| matches!(i, Insight::Inefficient { node, reasons, .. } if node == "fn:app/jobs.py#slow" && reasons[0].contains("blocking call time.sleep"))), "{insights:?}");
    assert!(!insights.iter().any(|i| matches!(i, Insight::Parallelisable { node, .. } if node == "fn:crates/api/src/store.rs#SessionStore::list")), "same receiver twice is not parallelisable: {insights:?}");
    assert!(!insights.iter().any(|i| matches!(i, Insight::Parallelisable { node, .. } if node == "fn:webui/src/api.ts#fetchSessions")), "{insights:?}");
    assert!(
        insights.iter().any(
            |i| matches!(i, Insight::Inefficient { node, .. } if node == "fn:app/jobs.py#fib")
        )
    );
}

#[test]
fn index_lookups() {
    let idx = build();
    assert_eq!(
        idx.function_at("crates/api/src/lib.rs", 33).unwrap().name,
        "list_sessions"
    );
    assert!(idx.function_at("crates/api/src/lib.rs", 200).is_none());
    assert_eq!(idx.functions_named("list").len(), 1);
    assert_eq!(
        idx.functions_named("crate::store::SessionStore::list")
            .len(),
        1
    );
    let hits = idx.search("session", 10);
    assert!(!hits.is_empty());
    assert!(hits.windows(2).all(|w| w[0].score >= w[1].score));
    assert_eq!(
        idx.search("createSession", 5)[0].node,
        "fn:webui/src/api.ts#createSession"
    );
    assert!(
        idx.entrypoints()
            .iter()
            .any(|n| n.id == "fn:crates/api/src/lib.rs#main")
    );
    assert_eq!(idx.node("mod:crates/api").unwrap().kind, NodeKind::Package);
    assert_eq!(
        idx.node("file:crates/api/src/lib.rs")
            .unwrap()
            .parent
            .as_deref(),
        Some("mod:crates/api")
    );
}

#[test]
fn save_and_load_roundtrip() {
    let graph = build_graph(Path::new("/repo"), fixture(), &Packs::builtin());
    let dir = std::env::temp_dir().join(format!("hoshizora-core-test-{}", std::process::id()));
    let path = dir.join("nested").join("graph.json");
    hoshizora_core::save(&graph, &path).unwrap();
    let loaded = hoshizora_core::load(&path).unwrap();
    assert_eq!(loaded, graph);
    hoshizora_core::save_with(&graph, &path, true).unwrap();
    assert_eq!(hoshizora_core::load(&path).unwrap(), graph);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn packs_override() {
    let dir = std::env::temp_dir().join(format!("hoshizora-packs-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("custom.toml"),
        "[[effect]]\nid = \"rust.tokio.spawn\"\nmatch = [\"never\"]\nkind = \"spawn\"\n\n[[effect]]\nid = \"custom.ping\"\nmatch = [\"ping\"]\nkind = \"network\"\n",
    )
    .unwrap();
    let mut packs = Packs::builtin();
    let before = packs.effects().count();
    packs.load_dir(&dir).unwrap();
    assert_eq!(packs.effects().count(), before + 1);
    assert_eq!(
        packs
            .effects()
            .find(|r| r.id == "rust.tokio.spawn")
            .unwrap()
            .patterns[0]
            .source(),
        "never"
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
#[ignore = "synthetic performance check; run with --ignored"]
fn bench_build_graph() {
    let files_n = 1000;
    let fns_per_file = 20;
    let calls_per_fn = 10;
    let mut files = Vec::with_capacity(files_n);
    for fi in 0..files_n {
        let mut symbols = Vec::with_capacity(fns_per_file);
        for si in 0..fns_per_file {
            let mut calls = Vec::with_capacity(calls_per_fn);
            for ci in 0..calls_per_fn {
                let target_file = (fi * 7 + ci * 13) % files_n;
                let target_fn = (si + ci) % fns_per_file;
                let mut c = call(
                    &format!("f{target_file}_{target_fn}"),
                    (si * 10 + ci) as u32 + 1,
                );
                if ci == 0 {
                    c = args(c, &["SELECT id FROM users WHERE id = $1"]);
                    c.callee = "sqlx::query".into();
                    c.name = "query".into();
                }
                calls.push(c);
            }
            symbols.push(func(
                &format!("f{fi}_{si}"),
                (si * 10) as u32 + 1,
                (si * 10) as u32 + 9,
                calls,
            ));
        }
        let imports = (0..5)
            .map(|k| import(&format!("crate::m{}", (fi + k + 1) % files_n), &[]))
            .chain([import("sqlx::PgPool", &[])])
            .collect();
        files.push(file(
            &format!("src/m{fi}.rs"),
            Language::Rust,
            &format!("crate::m{fi}"),
            imports,
            symbols,
        ));
    }
    let start = std::time::Instant::now();
    let graph = build_graph(Path::new("/bench"), files, &Packs::builtin());
    let built = start.elapsed();
    let fns = graph
        .nodes
        .iter()
        .filter(|n| n.kind == NodeKind::Function)
        .count();
    let start = std::time::Instant::now();
    let idx = Index::new(graph);
    let indexed = start.elapsed();
    let start = std::time::Instant::now();
    let insights = idx.static_insights().len();
    let flow = idx.flow("fn:src/m0.rs#f0_0", 6, &[]).unwrap();
    let queried = start.elapsed();
    eprintln!(
        "bench: {fns} functions, {} edges: build {built:?}, index {indexed:?}, insights+flow {queried:?} (flow nodes {}, insights {insights})",
        idx.graph().edges.len(),
        flow.nodes.len()
    );
    assert!(built.as_secs_f64() < 2.0, "build took {built:?}");
}

fn attrs(mut sym: Symbol, a: &[&str]) -> Symbol {
    sym.attributes = a.iter().map(|s| (*s).to_owned()).collect();
    sym
}

fn asyncf(mut sym: Symbol) -> Symbol {
    sym.is_async = true;
    sym
}

fn recv(mut c: CallSite, r: &str) -> CallSite {
    c.receiver = Some(r.into());
    c
}

fn conditional(mut c: CallSite) -> CallSite {
    c.conditional = true;
    c
}

fn sqlx_fn(local: &str, start: u32, sql: &str) -> Symbol {
    asyncf(func(
        local,
        start,
        start + 6,
        vec![
            args(call("sqlx::query_as", start + 1), &[sql]),
            awaited(call("fetch_all", start + 3)),
        ],
    ))
}

/// Shapes taken from examples/demo as the adapter emits them: crate-named modules, bare
/// `route`/`get` callees after dropped receivers, handler identifiers as call sites,
/// `format!` URLs with `{}` placeholders, `ui:*` attributes on template handlers.
fn demo_fixture() -> Vec<FileFacts> {
    let main_rs = file(
        "api/src/main.rs",
        Language::Rust,
        "observatory_api",
        vec![
            import("axum::routing", &["get", "post"]),
            import("axum::Router", &[]),
        ],
        vec![
            func("main", 24, 29, vec![call("run", 26)]),
            asyncf(func(
                "run",
                31,
                70,
                vec![
                    call("Router::new", 55),
                    args(call("route", 56), &["/health"]),
                    call("get", 56),
                    call("handlers::health", 56),
                    args(call("route", 57), &["/stars"]),
                    call("get", 57),
                    call("handlers::list_stars", 57),
                    args(call("route", 58), &["/observations"]),
                    call("post", 58),
                    call("handlers::launch_observation", 58),
                    args(call("route", 60), &["/catalog/summary"]),
                    call("get", 60),
                    call("handlers::catalog_summary", 60),
                ],
            )),
        ],
    );

    let launch_request = attrs(
        ty("LaunchRequest", 47, &[]),
        &[
            "derive(Deserialize, Debug)",
            "serde(default = \"default_site\")",
        ],
    );
    let handlers_rs = file(
        "api/src/handlers.rs",
        Language::Rust,
        "observatory_api::handlers",
        vec![
            import(
                "crate::catalog",
                &["distance_modulus", "find_duplicate_stars", "warm_lens"],
            ),
            import("crate::db", &["self", "Star"]),
            import(
                "crate::worker_client",
                &["compute_magnitudes", "fetch_weather"],
            ),
            import("crate::AppState", &[]),
        ],
        vec![
            asyncf(func("health", 20, 22, vec![])),
            asyncf(func(
                "list_stars",
                32,
                45,
                vec![
                    awaited(call("db::all_stars", 36)),
                    in_loop(awaited(call("db::observation_count_for_star", 39)), 1),
                    in_loop(call("out.push", 42), 1),
                ],
            )),
            launch_request,
            func("default_site", 53, 55, vec![]),
            asyncf(func(
                "launch_observation",
                58,
                100,
                vec![
                    awaited(call("db::star_by_name", 63)),
                    awaited(call("db::inventory", 69)),
                    awaited(call("fetch_weather", 70)),
                    call("warm_lens", 76),
                    awaited(call("db::insert_observation", 78)),
                    call("tokio::spawn", 85),
                    call("record_observation_event", 85),
                ],
            )),
            asyncf(func(
                "record_observation_event",
                103,
                108,
                vec![
                    awaited(call("tokio::time::sleep", 104)),
                    awaited(call("db::insert_event", 105)),
                ],
            )),
            asyncf(func(
                "catalog_summary",
                136,
                147,
                vec![
                    call("tokio::join!", 140),
                    call("db::count_stars", 141),
                    call("db::count_observations", 142),
                ],
            )),
            asyncf(func(
                "recompute_catalog",
                104,
                125,
                vec![
                    awaited(call("db::all_stars", 107)),
                    call("find_duplicate_stars", 108),
                    awaited(call("compute_magnitudes", 109)),
                    in_loop(awaited(call("db::update_magnitude", 112)), 1),
                ],
            )),
            asyncf(func(
                "conditional_case",
                150,
                160,
                vec![
                    awaited(call("db::inventory", 152)),
                    conditional(awaited(call("fetch_weather", 154))),
                ],
            )),
        ],
    );

    let db_rs = file(
        "api/src/db.rs",
        Language::Rust,
        "observatory_api::db",
        vec![import("sqlx", &["FromRow", "PgPool"])],
        vec![
            sqlx_fn("all_stars", 22, "SELECT id, name FROM stars ORDER BY id"),
            sqlx_fn(
                "observation_count_for_star",
                32,
                "SELECT count(*) FROM observations WHERE star_id = $1",
            ),
            sqlx_fn(
                "star_by_name",
                42,
                "SELECT id, name FROM stars WHERE name = $1",
            ),
            sqlx_fn(
                "inventory",
                52,
                "SELECT instrument, available FROM inventory",
            ),
            sqlx_fn(
                "insert_observation",
                62,
                "INSERT INTO observations (star_id, site, seeing) VALUES ($1, $2, $3) RETURNING id",
            ),
            sqlx_fn(
                "insert_event",
                76,
                "INSERT INTO events (topic, payload) VALUES ($1, $2)",
            ),
            sqlx_fn(
                "update_magnitude",
                88,
                "UPDATE stars SET magnitude = $2 WHERE id = $1",
            ),
            sqlx_fn("count_stars", 100, "SELECT count(*) FROM stars"),
            sqlx_fn(
                "count_observations",
                110,
                "SELECT count(*) FROM observations",
            ),
        ],
    );

    let catalog_rs = file(
        "api/src/catalog.rs",
        Language::Rust,
        "observatory_api::catalog",
        vec![import("crate::db::Star", &[])],
        vec![
            func(
                "find_duplicate_stars",
                7,
                20,
                vec![in_loop(call("dupes.push", 15), 2)],
            ),
            func("distance_modulus", 23, 27, vec![]),
            func("warm_lens", 30, 32, vec![call("std::thread::sleep", 31)]),
            func("legacy_magnitude_correction", 36, 38, vec![]),
        ],
    );

    let worker_client_rs = file(
        "api/src/worker_client.rs",
        Language::Rust,
        "observatory_api::worker_client",
        vec![
            import("crate::db::Star", &[]),
            import("crate::AppState", &[]),
        ],
        vec![
            asyncf(func(
                "fetch_weather",
                27,
                39,
                vec![
                    args(call("get", 32), &["{}/weather/{site}"]),
                    awaited(call("send", 34)),
                ],
            )),
            asyncf(func(
                "compute_magnitudes",
                42,
                55,
                vec![
                    args(call("post", 47), &["{}/compute/magnitudes"]),
                    awaited(call("send", 50)),
                ],
            )),
        ],
    );

    let worker_main_py = file(
        "worker/app/main.py",
        Language::Python,
        "app.main",
        vec![
            import("fastapi", &["FastAPI"]),
            import("app", &["db", "telemetry"]),
        ],
        vec![
            attrs(
                asyncf(func("health", 25, 27, vec![])),
                &["app.get(\"/health\")"],
            ),
            attrs(
                asyncf(func(
                    "weather",
                    45,
                    48,
                    vec![awaited(call("asyncio.gather", 47))],
                )),
                &["app.get(\"/weather/{site}\")"],
            ),
            attrs(
                asyncf(func(
                    "compute_magnitudes",
                    58,
                    62,
                    vec![awaited(args(
                        call("db.publish_event", 60),
                        &["catalog.recomputed"],
                    ))],
                )),
                &["app.post(\"/compute/magnitudes\")"],
            ),
        ],
    );

    let worker_db_py = file(
        "worker/app/db.py",
        Language::Python,
        "app.db",
        vec![import("psycopg", &[])],
        vec![asyncf(func(
            "recent_observations",
            19,
            27,
            vec![
                awaited(call("connect", 20)),
                awaited(args(
                    recv(call("conn.execute", 21), "conn"),
                    &["SELECT star_id, seeing FROM observations WHERE site = %s"],
                )),
                awaited(recv(call("cur.fetchall", 26), "cur")),
            ],
        ))],
    );

    let mut request = asyncf(func(
        "request",
        3,
        12,
        vec![awaited(args(call("fetch", 4), &["${API_URL}${path}"]))],
    ));
    request.visibility = Visibility::Private;
    let mut list_stars_ts = func(
        "listStars",
        14,
        16,
        vec![args(call("request", 15), &["/stars"])],
    );
    list_stars_ts.visibility = Visibility::Public;
    let mut launch_ts = func(
        "launchObservation",
        18,
        20,
        vec![args(call("request", 19), &["/observations", "POST"])],
    );
    launch_ts.visibility = Visibility::Public;
    let mut summary_ts = func(
        "catalogSummary",
        22,
        24,
        vec![args(call("request", 23), &["/catalog/summary"])],
    );
    summary_ts.visibility = Visibility::Public;
    let web_api_ts = file(
        "web/src/api.ts",
        Language::TypeScript,
        "src/api",
        vec![import("./config.ts", &["API_URL"])],
        vec![request, list_stars_ts, launch_ts, summary_ts],
    );
    let web_main_ts = file(
        "web/src/main.ts",
        Language::TypeScript,
        "src/main",
        vec![
            import("./api.ts", &["listStars", "launchObservation"]),
            import("./otel.js", &["initTelemetry"]),
        ],
        vec![
            attrs(
                func("onListStars", 34, 36, vec![call("listStars", 35)]),
                &["ui:click"],
            ),
            attrs(
                func("onLaunch", 26, 30, vec![call("launchObservation", 29)]),
                &["ui:click"],
            ),
            func("show", 9, 11, vec![]),
        ],
    );
    let web_otel_ts = file(
        "web/src/otel.ts",
        Language::TypeScript,
        "src/otel",
        vec![],
        vec![func("initTelemetry", 1, 5, vec![])],
    );
    let web_config_ts = file(
        "web/src/config.ts",
        Language::TypeScript,
        "src/config",
        vec![],
        vec![],
    );

    vec![
        main_rs,
        handlers_rs,
        db_rs,
        catalog_rs,
        worker_client_rs,
        worker_main_py,
        worker_db_py,
        web_api_ts,
        web_main_ts,
        web_otel_ts,
        web_config_ts,
    ]
}

fn build_demo() -> Index {
    Index::new(build_graph(
        Path::new("/demo"),
        demo_fixture(),
        &Packs::builtin(),
    ))
}

#[test]
fn demo_crate_imports_resolve_to_crate_named_modules() {
    let idx = build_demo();
    assert!(!edges_between(&idx, "file:api/src/handlers.rs", "file:api/src/db.rs").is_empty());
    assert!(!edges_between(&idx, "file:api/src/handlers.rs", "file:api/src/catalog.rs").is_empty());
    assert!(!edges_between(&idx, "file:api/src/catalog.rs", "file:api/src/db.rs").is_empty());
    assert!(!edges_between(&idx, "file:web/src/main.ts", "file:web/src/api.ts").is_empty());
    assert!(!edges_between(&idx, "file:web/src/main.ts", "file:web/src/otel.ts").is_empty());
    assert!(!edges_between(&idx, "file:web/src/api.ts", "file:web/src/config.ts").is_empty());
    let unresolved: Vec<&String> = idx
        .graph()
        .meta
        .warnings
        .iter()
        .filter(|w| w.starts_with("unresolved import"))
        .collect();
    assert!(unresolved.is_empty(), "{unresolved:?}");
    let e = edges_between(
        &idx,
        "fn:api/src/handlers.rs#list_stars",
        "fn:api/src/db.rs#observation_count_for_star",
    );
    assert_eq!(e.len(), 1);
    assert_eq!(e[0].confidence, hoshizora_model::Confidence::Exact);
}

#[test]
fn demo_bare_route_builder_and_handler_identifiers() {
    let idx = build_demo();
    assert!(
        edges_between(
            &idx,
            "route:GET /stars",
            "fn:api/src/handlers.rs#list_stars"
        )
        .iter()
        .any(|e| e.kind == EdgeKind::Handles)
    );
    assert!(
        edges_between(
            &idx,
            "route:POST /observations",
            "fn:api/src/handlers.rs#launch_observation"
        )
        .iter()
        .any(|e| e.kind == EdgeKind::Handles)
    );
    assert!(
        edges_between(
            &idx,
            "route:GET /catalog/summary",
            "fn:api/src/handlers.rs#catalog_summary"
        )
        .iter()
        .any(|e| e.kind == EdgeKind::Handles)
    );
    assert_eq!(
        idx.node("fn:api/src/handlers.rs#list_stars")
            .unwrap()
            .entrypoint,
        Some(EntrypointKind::HttpHandler)
    );
    let run = idx.node("fn:api/src/main.rs#run").unwrap();
    assert!(
        !run.effects
            .direct
            .iter()
            .any(|e| e.kind == EffectKind::Network),
        "bare get(handler) is not an HTTP client call: {:?}",
        run.effects.direct
    );
}

#[test]
fn demo_bare_client_get_with_format_url_links_route() {
    let idx = build_demo();
    let fw = idx
        .node("fn:api/src/worker_client.rs#fetch_weather")
        .unwrap();
    assert!(
        fw.effects
            .direct
            .iter()
            .any(|e| e.kind == EffectKind::Network && e.target.as_deref() == Some("/weather/{}")),
        "{:?}",
        fw.effects.direct
    );
    assert!(
        edges_between(&idx, &fw.id, "route:GET /weather/{}")
            .iter()
            .any(|e| e.kind == EdgeKind::Requests)
    );
    let launch = idx
        .node("fn:api/src/handlers.rs#launch_observation")
        .unwrap();
    assert!(launch.effects.transitive.contains(&EffectKind::Network));
}

#[test]
fn demo_io_in_loop_through_callee() {
    let idx = build_demo();
    let insights = idx.static_insights();
    assert!(
        insights.iter().any(|i| matches!(i, Insight::IoInLoop { node, line, effect, .. } if node == "fn:api/src/handlers.rs#list_stars" && *line == 39 && effect.contains("observation_count_for_star") && effect.contains("db_read"))),
        "{insights:?}"
    );
}

#[test]
fn demo_parallelisable_only_for_independent_awaits() {
    let idx = build_demo();
    let insights = idx.static_insights();
    let flagged: Vec<(&String, &Vec<String>)> = insights
        .iter()
        .filter_map(|i| match i {
            Insight::Parallelisable { node, children, .. } => Some((node, children)),
            _ => None,
        })
        .collect();
    let launch = flagged
        .iter()
        .find(|(n, _)| n.as_str() == "fn:api/src/handlers.rs#launch_observation")
        .expect("planted sequential awaits");
    assert_eq!(
        launch.1,
        &vec!["inventory:69".to_owned(), "fetch_weather:70".to_owned()]
    );
    for not_expected in [
        "catalog_summary",
        "recompute_catalog",
        "conditional_case",
        "recent_observations",
        "record_observation_event",
        "request",
    ] {
        assert!(
            !flagged
                .iter()
                .any(|(n, _)| n.ends_with(&format!("#{not_expected}"))),
            "{not_expected} flagged: {flagged:?}"
        );
    }
}

#[test]
fn demo_blocking_through_sync_helper() {
    let idx = build_demo();
    let launch = idx
        .node("fn:api/src/handlers.rs#launch_observation")
        .unwrap();
    assert!(
        launch.effects.transitive.contains(&EffectKind::Blocking),
        "{:?}",
        launch.effects.transitive
    );
    let warm = idx.node("fn:api/src/catalog.rs#warm_lens").unwrap();
    assert!(warm.effects.transitive.contains(&EffectKind::Sleep));
    assert!(!warm.effects.transitive.contains(&EffectKind::Blocking));
    let insights = idx.static_insights();
    assert!(
        insights.iter().any(|i| matches!(i, Insight::Inefficient { node, reasons, .. } if node == "fn:api/src/handlers.rs#launch_observation" && reasons.iter().any(|r| r.contains("blocking call in async context via warm_lens")))),
        "{insights:?}"
    );
    assert!(!insights.iter().any(|i| matches!(i, Insight::Inefficient { node, reasons, .. } if node == "fn:api/src/handlers.rs#record_observation_event" && reasons.iter().any(|r| r.contains("blocking")))));
}

#[test]
fn demo_spawn_argument_on_same_line() {
    let idx = build_demo();
    let e = edges_between(
        &idx,
        "fn:api/src/handlers.rs#launch_observation",
        "fn:api/src/handlers.rs#record_observation_event",
    );
    assert_eq!(e.len(), 1);
    assert_eq!(e[0].kind, EdgeKind::Spawns);
}

#[test]
fn demo_ui_attribute_entrypoints_and_dead() {
    let idx = build_demo();
    assert_eq!(
        idx.node("fn:web/src/main.ts#onListStars")
            .unwrap()
            .entrypoint,
        Some(EntrypointKind::UiHandler)
    );
    let dead: Vec<&str> = idx.dead().iter().map(|n| n.id.as_str()).collect();
    assert!(
        dead.contains(&"fn:api/src/catalog.rs#legacy_magnitude_correction"),
        "{dead:?}"
    );
    assert!(dead.contains(&"fn:web/src/main.ts#show"), "{dead:?}");
    for alive in [
        "default_site",
        "list_stars",
        "launch_observation",
        "warm_lens",
        "record_observation_event",
        "request",
        "listStars",
        "onListStars",
        "weather",
        "fetch_weather",
    ] {
        assert!(
            !dead.iter().any(|d| d.ends_with(&format!("#{alive}"))),
            "{alive} should be alive: {dead:?}"
        );
    }
}

#[test]
fn demo_client_wrappers_link_callers_to_routes() {
    let idx = build_demo();
    let req = |from: &str| -> Vec<String> {
        idx.outgoing(from)
            .into_iter()
            .filter(|e| e.kind == EdgeKind::Requests)
            .map(|e| e.to.clone())
            .collect()
    };
    assert_eq!(
        req("fn:web/src/api.ts#listStars"),
        vec!["route:GET /stars".to_owned()]
    );
    assert_eq!(
        req("fn:web/src/api.ts#launchObservation"),
        vec!["route:POST /observations".to_owned()]
    );
    assert_eq!(
        req("fn:web/src/api.ts#catalogSummary"),
        vec!["route:GET /catalog/summary".to_owned()]
    );
    assert!(
        req("fn:web/src/api.ts#request").is_empty(),
        "the wrapper itself has no literal path"
    );
    let launch = idx.node("fn:web/src/main.ts#onLaunch").unwrap();
    assert!(
        launch.effects.transitive.contains(&EffectKind::DbWrite),
        "{:?}",
        launch.effects.transitive
    );
    assert!(launch.effects.transitive.contains(&EffectKind::Blocking));
    let list = idx.node("fn:web/src/api.ts#listStars").unwrap();
    assert!(
        list.effects
            .direct
            .iter()
            .any(|e| e.kind == EffectKind::Network && e.target.as_deref() == Some("/stars"))
    );
    let flow = idx.flow("fn:web/src/main.ts#onLaunch", 8, &[]).unwrap();
    assert!(
        flow.nodes.contains(&"table:observations".to_owned()),
        "{:?}",
        flow.nodes
    );
}

#[test]
fn demo_publish_topic_and_cross_service_effects() {
    let idx = build_demo();
    assert_eq!(
        idx.node("topic:catalog.recomputed").unwrap().kind,
        NodeKind::Topic
    );
    assert!(
        edges_between(
            &idx,
            "fn:worker/app/main.py#compute_magnitudes",
            "topic:catalog.recomputed"
        )
        .iter()
        .any(|e| e.kind == EdgeKind::Publishes)
    );
    assert!(
        edges_between(
            &idx,
            "fn:api/src/worker_client.rs#compute_magnitudes",
            "route:POST /compute/magnitudes"
        )
        .iter()
        .any(|e| e.kind == EdgeKind::Requests)
    );
    let recompute = idx
        .node("fn:api/src/handlers.rs#recompute_catalog")
        .unwrap();
    assert!(
        recompute.effects.transitive.contains(&EffectKind::Publish),
        "{:?}",
        recompute.effects.transitive
    );
    let insights = idx.static_insights();
    assert!(insights.iter().any(|i| matches!(i, Insight::IoInLoop { node, line, .. } if node == &recompute.id && *line == 112)));
}
