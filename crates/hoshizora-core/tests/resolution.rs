use std::path::Path;

use hoshizora_core::{Index, Packs, build_graph};
use hoshizora_model::{
    Body, CallSite, Confidence, EdgeKind, EffectKind, EntrypointKind, FileFacts, Import, Language,
    NodeKind, Param, Span, Symbol, SymbolKind, Visibility,
};

fn call(callee: &str, line: u32) -> CallSite {
    CallSite {
        callee: callee.into(),
        name: callee.rsplit(['.', ':']).next().unwrap_or(callee).into(),
        receiver: None,
        string_args: vec![],
        line,
        loop_depth: 0,
        awaited: true,
        conditional: false,
        in_closure: false,
        binds: vec![],
        uses: vec![],
        mutates: vec![],
    }
}

fn args(mut c: CallSite, a: &[&str]) -> CallSite {
    c.string_args = a.iter().map(|s| (*s).to_owned()).collect();
    c
}

fn recv(mut c: CallSite, r: &str) -> CallSite {
    c.receiver = Some(r.into());
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

fn symbol(kind: SymbolKind, local: &str, start: u32, calls: Option<Vec<CallSite>>) -> Symbol {
    let (parent, name) = match local.rsplit_once("::").or_else(|| local.rsplit_once('.')) {
        Some((p, n)) => (Some(p.to_owned()), n.to_owned()),
        None => (None, local.to_owned()),
    };
    Symbol {
        kind,
        name,
        local_name: local.into(),
        parent,
        signature: local.into(),
        params: vec![],
        returns: None,
        doc: None,
        span: Span {
            start_line: start,
            end_line: start + 20,
        },
        is_async: true,
        visibility: Visibility::Public,
        attributes: vec![],
        implements: vec![],
        body: calls.map(|calls| Body {
            calls,
            cyclomatic: 1,
            ..Body::default()
        }),
    }
}

fn func(local: &str, start: u32, calls: Vec<CallSite>) -> Symbol {
    let kind = if local.contains(['.', ':']) {
        SymbolKind::Method
    } else {
        SymbolKind::Function
    };
    symbol(kind, local, start, Some(calls))
}

fn sync(mut s: Symbol) -> Symbol {
    s.is_async = false;
    s
}

fn with_params(mut s: Symbol, params: &[(&str, &str)]) -> Symbol {
    s.params = params
        .iter()
        .map(|(n, t)| Param {
            name: (*n).into(),
            ty: Some((*t).into()),
            default: None,
        })
        .collect();
    s
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
        lines: 100,
        imports,
        symbols,
        domains: vec![],
    }
}

fn build(files: Vec<FileFacts>) -> Index {
    Index::new(build_graph(Path::new("/repo"), files, &Packs::builtin()))
}

fn edge<'a>(idx: &'a Index, from: &str, to: &str) -> Option<&'a hoshizora_model::Edge> {
    idx.outgoing(from).into_iter().find(|e| e.to == to)
}

/// Frontend: `sf.actions.spawn()` through a class field initialised by a factory, and
/// `api.postForm("/sessions/spawn")` as a client call of the route.
fn frontend() -> Vec<FileFacts> {
    let ts = Language::TypeScript;
    let form_type = with_params(
        symbol(SymbolKind::Type, "SpawnForm", 1, None),
        &[("actions", "useSessionActions()"), ("client", "Client")],
    );
    vec![
        file(
            "webui/src/spawnForm.ts",
            ts,
            "webui/src/spawnForm",
            vec![],
            vec![
                form_type,
                func(
                    "SpawnForm.submit",
                    10,
                    vec![recv(call("this.actions.spawn", 12), "this.actions")],
                ),
            ],
        ),
        file(
            "webui/src/spawnSubmit.ts",
            ts,
            "webui/src/spawnSubmit",
            vec![import("./spawnForm", &["SpawnForm"])],
            vec![with_params(
                func(
                    "spawnOnMachine",
                    5,
                    vec![recv(call("sf.actions.spawn", 7), "sf.actions")],
                ),
                &[("sf", "SpawnForm")],
            )],
        ),
        file(
            "webui/src/sessionActions.ts",
            ts,
            "webui/src/sessionActions",
            vec![import("./endpoints", &["endpoints"])],
            vec![
                func("useSessionActions", 1, vec![]),
                func(
                    "useSessionActions.spawn",
                    3,
                    vec![call("endpoints.spawn", 3)],
                ),
            ],
        ),
        file(
            "webui/src/endpoints.ts",
            ts,
            "webui/src/endpoints",
            vec![import("./api", &["api"])],
            vec![func(
                "endpoints.spawn",
                20,
                vec![recv(
                    args(call("api.postForm", 24), &["/sessions/spawn"]),
                    "api",
                )],
            )],
        ),
        file(
            "webui/src/api.ts",
            ts,
            "webui/src/api",
            vec![],
            vec![func("postForm", 1, vec![])],
        ),
    ]
}

/// Backend: a project-specific registration wrapper spanning several lines, handlers named
/// through module paths, sqlx used fully qualified without a `use`.
fn backend() -> Vec<FileFacts> {
    let rs = Language::Rust;
    vec![
        file(
            "crates/server/src/lib.rs",
            rs,
            "server",
            vec![],
            vec![func("main", 1, vec![])],
        ),
        file(
            "crates/server/src/api_routes.rs",
            rs,
            "server::api_routes",
            vec![
                import("crate", &["routes"]),
                import("axum::routing::post", &[]),
            ],
            vec![
                func(
                    "register",
                    10,
                    vec![
                        args(
                            recv(call("r.add", 11), "r"),
                            &["/sessions/spawn", "Spawn a session."],
                        ),
                        call("post", 15),
                        call("routes::spawn::spawn_session", 15),
                        call("body_limit", 15),
                        args(call("add", 20), &["/sessions/{id}/kill", "Kill it."]),
                        call("post", 24),
                        call("crate::routes::spawn::kill_session", 24),
                    ],
                ),
                sync(func("body_limit", 40, vec![])),
            ],
        ),
        file(
            "crates/server/src/routes/mod.rs",
            rs,
            "server::routes",
            vec![],
            vec![],
        ),
        file(
            "crates/server/src/routes/spawn.rs",
            rs,
            "server::routes::spawn",
            vec![],
            vec![
                func("spawn_session", 1, vec![call("save_draft", 5)]),
                func("kill_session", 30, vec![]),
                func(
                    "save_draft",
                    50,
                    vec![
                        args(
                            call("sqlx::query", 52),
                            &["INSERT INTO sessions (id) VALUES ($1)"],
                        ),
                        awaited_exec(),
                    ],
                ),
            ],
        ),
    ]
}

fn awaited_exec() -> CallSite {
    call("sqlx::query(..).execute", 53)
}

/// Messaging: an enum variant handed to a method call in one crate, matched on in another.
fn messaging() -> Vec<FileFacts> {
    let rs = Language::Rust;
    let message_site =
        |variant: &str, line: u32| recv(args(call("<message>", line), &[variant]), "bus");
    let match_site = |variant: &str, line: u32| args(call("<match>", line), &[variant]);
    vec![
        file(
            "crates/proto/src/lib.rs",
            rs,
            "proto",
            vec![],
            vec![symbol(SymbolKind::Type, "FrameDown", 1, None)],
        ),
        file(
            "crates/server/src/launch.rs",
            rs,
            "server::launch",
            vec![],
            vec![func(
                "launch_machine",
                1,
                vec![
                    call("bus.command_daemon", 5),
                    message_site("FrameDown::Command", 5),
                    message_site("Value::Object", 6),
                    message_site("FrameDown::Unmatched", 7),
                ],
            )],
        ),
        file(
            "crates/daemon/src/supervisor.rs",
            rs,
            "daemon::supervisor",
            vec![],
            vec![func(
                "handle_frame",
                1,
                vec![
                    match_site("FrameDown::Command", 4),
                    match_site("Value::Object", 5),
                ],
            )],
        ),
    ]
}

#[test]
fn field_typed_receivers_resolve_through_factories_and_imports() {
    let idx = build(frontend());
    let via_param = edge(
        &idx,
        "fn:webui/src/spawnSubmit.ts#spawnOnMachine",
        "fn:webui/src/sessionActions.ts#useSessionActions.spawn",
    )
    .expect("sf.actions.spawn resolves through the field's factory");
    assert_eq!(via_param.confidence, Confidence::Exact);
    let via_this = edge(
        &idx,
        "fn:webui/src/spawnForm.ts#SpawnForm.submit",
        "fn:webui/src/sessionActions.ts#useSessionActions.spawn",
    )
    .expect("this.actions.spawn resolves the same way");
    assert_eq!(via_this.kind, EdgeKind::Calls);
    assert!(
        edge(
            &idx,
            "fn:webui/src/sessionActions.ts#useSessionActions.spawn",
            "fn:webui/src/endpoints.ts#endpoints.spawn"
        )
        .is_some(),
        "object-literal method reached through its import"
    );
}

#[test]
fn camel_case_client_methods_request_routes() {
    let mut files = frontend();
    files.extend(backend());
    let idx = build(files);
    let req = edge(
        &idx,
        "fn:webui/src/endpoints.ts#endpoints.spawn",
        "route:POST /sessions/spawn",
    )
    .expect("api.postForm issues a POST request");
    assert_eq!(req.kind, EdgeKind::Requests);
    let node = idx
        .node("fn:webui/src/endpoints.ts#endpoints.spawn")
        .unwrap();
    assert!(
        node.effects.direct.iter().any(
            |e| e.kind == EffectKind::Network && e.target.as_deref() == Some("/sessions/spawn")
        )
    );
}

#[test]
fn wrapper_registrations_resolve_module_path_handlers() {
    let idx = build(backend());
    let handles = edge(
        &idx,
        "route:POST /sessions/spawn",
        "fn:crates/server/src/routes/spawn.rs#spawn_session",
    )
    .expect("multi-line r.add(..., post(routes::spawn::spawn_session)) registers the route");
    assert_eq!(handles.kind, EdgeKind::Handles);
    assert!(
        edge(
            &idx,
            "route:POST /sessions/spawn",
            "fn:crates/server/src/api_routes.rs#body_limit"
        )
        .is_none(),
        "a sync config call alongside is not a handler"
    );
    assert!(
        edge(
            &idx,
            "route:POST /sessions/{}/kill",
            "fn:crates/server/src/routes/spawn.rs#kill_session"
        )
        .is_some(),
        "crate::routes::spawn::kill_session resolves through the module path"
    );
    let handler = idx
        .node("fn:crates/server/src/routes/spawn.rs#spawn_session")
        .unwrap();
    assert_eq!(handler.entrypoint, Some(EntrypointKind::HttpHandler));
    let call = edge(
        &idx,
        "fn:crates/server/src/api_routes.rs#register",
        "fn:crates/server/src/routes/spawn.rs#spawn_session",
    )
    .unwrap();
    assert_eq!(call.confidence, Confidence::Exact);
}

#[test]
fn qualified_callee_satisfies_requires_import() {
    let idx = build(backend());
    let draft = idx
        .node("fn:crates/server/src/routes/spawn.rs#save_draft")
        .unwrap();
    assert!(
        draft
            .effects
            .direct
            .iter()
            .any(|e| e.kind == EffectKind::DbWrite && e.target.as_deref() == Some("sessions")),
        "{:?}",
        draft.effects.direct
    );
    assert!(
        edge(
            &idx,
            "fn:crates/server/src/routes/spawn.rs#save_draft",
            "table:sessions"
        )
        .is_some_and(|e| e.kind == EdgeKind::Writes)
    );
}

#[test]
fn enum_messages_become_topics_when_sent_and_matched() {
    let idx = build(messaging());
    let topic = idx
        .node("topic:FrameDown::Command")
        .expect("sent and matched");
    assert_eq!(topic.kind, NodeKind::Topic);
    assert!(
        edge(
            &idx,
            "fn:crates/server/src/launch.rs#launch_machine",
            "topic:FrameDown::Command"
        )
        .is_some_and(|e| e.kind == EdgeKind::Publishes)
    );
    assert!(
        edge(
            &idx,
            "fn:crates/daemon/src/supervisor.rs#handle_frame",
            "topic:FrameDown::Command"
        )
        .is_some_and(|e| e.kind == EdgeKind::Subscribes)
    );
    assert!(
        idx.node("topic:FrameDown::Unmatched").is_none(),
        "one-sided"
    );
    assert!(
        idx.node("topic:Value::Object").is_none(),
        "third-party enum"
    );
    let publisher = idx
        .node("fn:crates/server/src/launch.rs#launch_machine")
        .unwrap();
    let published: Vec<_> = publisher
        .effects
        .direct
        .iter()
        .filter(|e| e.kind == EffectKind::Publish)
        .map(|e| e.target.clone().unwrap_or_default())
        .collect();
    assert_eq!(published, vec!["FrameDown::Command"]);
}

#[test]
fn middleware_after_the_last_registration_is_not_a_handler() {
    let rs = Language::Rust;
    let ts = Language::TypeScript;
    let idx = build(vec![
        file(
            "api/src/main.rs",
            rs,
            "api",
            vec![],
            vec![
                func(
                    "router",
                    10,
                    vec![
                        args(
                            recv(call("Router::new().route", 11), "Router::new()"),
                            &["/stars"],
                        ),
                        call("get", 11),
                        call("handlers::list_stars", 11),
                        args(recv(call("route", 12), "_"), &["/catalog/summary"]),
                        call("get", 12),
                        call("handlers::catalog_summary", 12),
                        recv(call("layer", 13), "_"),
                        call("TraceLayer::new_for_http", 13),
                        call("make_span_with", 13),
                        call("telemetry::http_server_span", 13),
                        recv(call("layer", 14), "_"),
                        call("middleware::from_fn", 14),
                        call("telemetry::authenticate", 14),
                        recv(call("fallback", 15), "_"),
                        call("any", 15),
                        call("handlers::not_found", 15),
                        args(recv(call("route", 20), "_"), &["/upload"]),
                        call("post", 22),
                        call("handlers::upload", 22),
                        recv(call("layer", 22), "_"),
                        call("DefaultBodyLimit::max", 22),
                        call("body_limit", 22),
                    ],
                ),
                func("list_stars", 40, vec![]),
                func("catalog_summary", 50, vec![]),
                func("http_server_span", 60, vec![]),
                func("authenticate", 70, vec![]),
                func("not_found", 80, vec![]),
                func("upload", 90, vec![]),
                sync(func("body_limit", 100, vec![])),
            ],
        ),
        file(
            "web/src/app.ts",
            ts,
            "web/src/app",
            vec![],
            vec![
                func(
                    "start",
                    1,
                    vec![
                        args(recv(call("app.get", 2), "app"), &["/users"]),
                        call("listUsers", 2),
                        recv(call("app.use", 3), "app"),
                        call("requestLogger", 3),
                    ],
                ),
                func("listUsers", 20, vec![]),
                func("requestLogger", 30, vec![]),
            ],
        ),
    ]);
    for (route, handler) in [
        ("route:GET /stars", "fn:api/src/main.rs#list_stars"),
        (
            "route:GET /catalog/summary",
            "fn:api/src/main.rs#catalog_summary",
        ),
        ("route:POST /upload", "fn:api/src/main.rs#upload"),
        ("route:GET /users", "fn:web/src/app.ts#listUsers"),
    ] {
        assert!(
            edge(&idx, route, handler).is_some_and(|e| e.kind == EdgeKind::Handles),
            "{route} keeps its handler {handler}"
        );
    }
    for middleware in [
        "fn:api/src/main.rs#http_server_span",
        "fn:api/src/main.rs#authenticate",
        "fn:api/src/main.rs#not_found",
        "fn:api/src/main.rs#body_limit",
        "fn:web/src/app.ts#requestLogger",
    ] {
        assert!(
            idx.incoming(middleware)
                .into_iter()
                .all(|e| e.kind != EdgeKind::Handles),
            "{middleware} is middleware, not a handler"
        );
    }
}

/// `next.run(req)` on a parameter of an external type is a method call: it must not bind to
/// a free `run` by name. Calls through an imported module namespace still do.
#[test]
fn value_receivers_do_not_bind_free_functions_by_name() {
    let idx = build(vec![
        file(
            "api/src/main.rs",
            Language::Rust,
            "api/src/main",
            vec![],
            vec![func("run", 1, vec![])],
        ),
        file(
            "api/src/telemetry.rs",
            Language::Rust,
            "api/src/telemetry",
            vec![],
            vec![with_params(
                func(
                    "http_server_span",
                    1,
                    vec![recv(call("next.run", 3), "next")],
                ),
                &[("req", "Request"), ("next", "Next")],
            )],
        ),
        file(
            "web/src/api.ts",
            Language::TypeScript,
            "web/src/api",
            vec![],
            vec![func("load", 1, vec![])],
        ),
        file(
            "web/src/page.ts",
            Language::TypeScript,
            "web/src/page",
            vec![Import {
                alias: Some("api".into()),
                ..import("./api", &[])
            }],
            vec![func("render", 1, vec![recv(call("api.load", 2), "api")])],
        ),
        file(
            "svc/util.py",
            Language::Python,
            "svc/util",
            vec![],
            vec![func("f", 1, vec![])],
        ),
        file(
            "svc/main.py",
            Language::Python,
            "svc/main",
            vec![import("util", &[])],
            vec![func("main", 1, vec![recv(call("util.f", 2), "util")])],
        ),
        file(
            "cmd/pkg/pkg.go",
            Language::Go,
            "cmd/pkg",
            vec![],
            vec![func("F", 1, vec![])],
        ),
        file(
            "cmd/main.go",
            Language::Go,
            "cmd",
            vec![import("example.com/cmd/pkg", &[])],
            vec![func("main", 1, vec![recv(call("pkg.F", 2), "pkg")])],
        ),
    ]);
    assert!(
        edge(
            &idx,
            "fn:api/src/telemetry.rs#http_server_span",
            "fn:api/src/main.rs#run"
        )
        .is_none(),
        "next.run() does not bind the free fn run"
    );
    for (from, to) in [
        ("fn:web/src/page.ts#render", "fn:web/src/api.ts#load"),
        ("fn:svc/main.py#main", "fn:svc/util.py#f"),
        ("fn:cmd/main.go#main", "fn:cmd/pkg/pkg.go#F"),
    ] {
        assert!(
            edge(&idx, from, to).is_some_and(|e| e.kind == EdgeKind::Calls),
            "{from} still calls {to} through the module namespace"
        );
    }
}

/// A method call on a receiver of unknown type binds by name only when the name is not a
/// common trait/lifecycle name (unique: plain edge; several: dimmed candidates), or when the
/// receiver names the target type. Typed receivers resolve exactly as before.
#[test]
fn untyped_receivers_bind_only_distinctive_unique_methods() {
    let idx = build(vec![
        file(
            "api/src/watchdog.rs",
            Language::Rust,
            "api/src/watchdog",
            vec![],
            vec![
                func("Watchdog::tick", 1, vec![]),
                func("Watchdog::reconcile_leases", 30, vec![]),
                func("Watchdog::describe_thing", 60, vec![]),
            ],
        ),
        file(
            "api/src/durable.rs",
            Language::Rust,
            "api/src/durable",
            vec![],
            vec![
                func("Durable::forget", 1, vec![]),
                func("Durable::describe_thing", 30, vec![]),
                func("Connection::process", 60, vec![]),
            ],
        ),
        file(
            "api/src/main.rs",
            Language::Rust,
            "api/src/main",
            vec![],
            vec![
                func(
                    "run",
                    1,
                    vec![
                        recv(call("tick.tick", 2), "tick"),
                        recv(call("<expr>.forget", 3), "<expr>"),
                        recv(call("<expr>.reconcile_leases", 4), "<expr>"),
                        recv(call("thing.describe_thing", 5), "thing"),
                        recv(call("watchdog.tick", 6), "watchdog"),
                        recv(call("conn.process", 7), "conn"),
                        recv(call("<expr>.process", 8), "<expr>"),
                    ],
                ),
                with_params(
                    func("typed", 40, vec![recv(call("w.tick", 41), "w")]),
                    &[("w", "Watchdog")],
                ),
            ],
        ),
    ]);
    let run = "fn:api/src/main.rs#run";
    let tick = "fn:api/src/watchdog.rs#Watchdog::tick";
    assert!(
        edge(&idx, run, "fn:api/src/durable.rs#Durable::forget").is_none(),
        "<expr>.forget() does not bind the only forget: the name is common"
    );
    let distinctive = edge(
        &idx,
        run,
        "fn:api/src/watchdog.rs#Watchdog::reconcile_leases",
    )
    .expect("<expr>.reconcile_leases() binds the unique distinctive method");
    assert_eq!(distinctive.confidence, Confidence::Name);
    for to in [
        "fn:api/src/watchdog.rs#Watchdog::describe_thing",
        "fn:api/src/durable.rs#Durable::describe_thing",
    ] {
        let e = edge(&idx, run, to).expect("thing.describe_thing() keeps both candidates");
        assert_eq!(e.attrs.get("candidates").map(String::as_str), Some("2"));
    }
    let process = "fn:api/src/durable.rs#Connection::process";
    let abbreviated: Vec<Option<u32>> = idx
        .outgoing(run)
        .iter()
        .filter(|e| e.to == process)
        .map(|e| e.line)
        .collect();
    assert_eq!(
        abbreviated,
        vec![Some(7)],
        "conn.process() binds through the abbreviated receiver; <expr>.process() does not"
    );
    let named = edge(&idx, run, tick).expect("watchdog.tick() binds through the receiver name");
    assert_eq!(
        named.attrs.get("receiver").map(String::as_str),
        Some("watchdog")
    );
    assert!(
        !idx.outgoing(run)
            .iter()
            .any(|e| e.to == tick && e.line == Some(2)),
        "tick.tick() on an unknown type does not bind Watchdog::tick"
    );
    let typed =
        edge(&idx, "fn:api/src/main.rs#typed", tick).expect("w: Watchdog resolves w.tick()");
    assert_eq!(typed.confidence, Confidence::Exact);
    assert_eq!(
        typed.attrs.get("receiver_type").map(String::as_str),
        Some("Watchdog")
    );
}
