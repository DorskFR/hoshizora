use std::path::Path;

use hoshizora_adapters::{
    Adapter, Flavor, GoAdapter, PythonAdapter, RustAdapter, SfcAdapter, TsAdapter, index_tree,
    module_path,
};
use hoshizora_model::{CallSite, FileFacts, Language, Symbol, SymbolKind, Visibility};

fn sym<'a>(facts: &'a FileFacts, local_name: &str) -> &'a Symbol {
    facts
        .symbols
        .iter()
        .find(|s| s.local_name == local_name)
        .unwrap_or_else(|| panic!("missing symbol {local_name}"))
}

fn call<'a>(s: &'a Symbol, callee: &str) -> &'a CallSite {
    s.body
        .as_ref()
        .expect("body")
        .calls
        .iter()
        .find(|c| c.callee == callee)
        .unwrap_or_else(|| panic!("missing call {callee} in {}", s.local_name))
}

fn names(s: &Symbol) -> Vec<&str> {
    s.body
        .as_ref()
        .expect("body")
        .calls
        .iter()
        .map(|c| c.callee.as_str())
        .collect()
}

#[test]
fn rust_axum_handler() {
    let facts = RustAdapter
        .parse(
            "src/api/sessions.rs",
            include_str!("fixtures/axum.rs"),
            "app::api::sessions",
        )
        .unwrap();
    assert_eq!(facts.language, Language::Rust);
    assert_eq!(facts.module, "app::api::sessions");
    assert_eq!(facts.lines, 100);

    let imports: Vec<_> = facts
        .imports
        .iter()
        .map(|i| (i.path.as_str(), i.names.clone()))
        .collect();
    assert_eq!(
        imports[0],
        ("axum", vec!["Json".into(), "extract::State".into()])
    );
    assert_eq!(imports[1], ("sqlx::PgPool", vec![]));
    assert_eq!(
        imports[2].1,
        vec!["Session".to_string(), "User as U".to_string()]
    );

    let store = sym(&facts, "SessionStore");
    assert_eq!(store.kind, SymbolKind::Type);
    assert_eq!(store.doc.as_deref(), Some("Stored session."));
    assert_eq!(store.attributes, vec!["derive(Debug, Clone)"]);
    assert_eq!(store.implements, vec!["Store"]);

    assert_eq!(sym(&facts, "Store").kind, SymbolKind::Interface);
    let trait_get = sym(&facts, "Store::get");
    assert_eq!(trait_get.doc.as_deref(), Some("Fetch one."));
    assert!(trait_get.body.is_none());
    assert_eq!(
        sym(&facts, "Store::describe").body.as_ref().unwrap().calls[0].callee,
        "format!"
    );

    let refresh = sym(&facts, "SessionStore::refresh");
    assert_eq!(refresh.kind, SymbolKind::Method);
    assert_eq!(refresh.parent.as_deref(), Some("SessionStore"));
    assert!(refresh.is_async);
    assert_eq!(refresh.visibility, Visibility::Public);
    assert_eq!(refresh.attributes, vec!["tracing::instrument(skip(self))"]);
    assert_eq!(
        refresh.doc.as_deref(),
        Some("Loads every session and refreshes the owners.\n\nRuns one query per user.")
    );
    assert_eq!(
        refresh.signature,
        "pub async fn refresh(&self, user_ids: &[i64], limit: Option<u32>) -> anyhow::Result<Vec<Session>>"
    );
    assert_eq!(refresh.params.len(), 3);
    assert_eq!(refresh.params[1].name, "user_ids");
    assert_eq!(refresh.params[1].ty.as_deref(), Some("&[i64]"));
    assert_eq!(
        refresh.returns.as_deref(),
        Some("anyhow::Result<Vec<Session>>")
    );
    assert_eq!((refresh.span.start_line, refresh.span.end_line), (32, 53));

    let query = call(refresh, "sqlx::query_as!");
    assert_eq!(query.name, "query_as!");
    assert_eq!(
        query.string_args,
        vec!["SELECT * FROM sessions WHERE user_id = $1"]
    );
    assert_eq!(query.loop_depth, 1);
    assert!(query.awaited);
    assert!(!query.conditional);
    let fetch = call(refresh, "fetch_one");
    assert!(fetch.awaited && fetch.loop_depth == 1 && fetch.receiver.as_deref() == Some("<expr>"));
    let warn = call(refresh, "tracing::warn!");
    assert!(warn.conditional);
    assert_eq!(warn.string_args, vec!["inactive {}"]);
    let get = call(refresh, "reqwest::get");
    assert!(get.in_closure && get.awaited && get.loop_depth == 0);
    assert_eq!(get.string_args, vec!["https://example.com/ping"]);
    assert!(!call(refresh, "tokio::spawn").in_closure);
    assert_eq!(call(refresh, "out.iter").receiver.as_deref(), Some("out"));
    assert_eq!(call(refresh, "out.iter").loop_depth, 0);
    assert!(call(refresh, "s.name.clone").in_closure);
    assert!(call(refresh, "out.truncate").conditional);

    let body = refresh.body.as_ref().unwrap();
    assert_eq!((body.loops, body.max_loop_depth), (1, 1));
    assert_eq!(body.branches, 2);
    assert_eq!(body.cyclomatic, 4);
    assert_eq!(body.early_exits, 1);
    assert_eq!(body.closures, 2);

    let list = sym(&facts, "list");
    assert_eq!(list.kind, SymbolKind::Function);
    assert_eq!(list.doc.as_deref(), Some("GET /sessions"));
    assert!(call(list, "store.refresh").awaited);

    let main = sym(&facts, "main");
    assert_eq!(main.attributes, vec!["tokio::main"]);
    assert_eq!(main.visibility, Visibility::Private);
    assert_eq!(call(main, "route").string_args, vec!["/sessions"]);
    let handler = call(main, "list");
    assert_eq!((handler.line, handler.name.as_str()), (64, "list"));
    assert!(handler.string_args.is_empty());
    assert!(!names(main).contains(&"app"));

    let router = sym(&facts, "router");
    assert_eq!(
        call(router, "client.get").string_args,
        vec!["{}/weather/{}"]
    );
    assert!(names(router).contains(&"format!"));
    let concat = router
        .body
        .as_ref()
        .unwrap()
        .calls
        .iter()
        .filter(|c| c.callee == "client.get")
        .nth(1)
        .unwrap();
    assert_eq!(concat.string_args, vec!["ab"]);
    let routes: Vec<_> = router
        .body
        .as_ref()
        .unwrap()
        .calls
        .iter()
        .filter(|c| c.callee == "route")
        .collect();
    assert_eq!(routes.len(), 2);
    assert_eq!(
        (routes[0].line, routes[0].string_args.clone()),
        (73, vec!["/stars".to_string()])
    );
    assert_eq!(routes[1].line, 74);
    assert_eq!(call(router, "list_stars").line, 73);
    assert_eq!(call(router, "delete_star").line, 74);
    assert_eq!(call(router, "axum::Router::new").line, 72);

    assert!(!facts.symbols.iter().any(|s| s.name == "<module>"));
    let observation = sym(&facts, "Observation");
    assert_eq!(
        observation.attributes,
        vec![
            "derive(Deserialize)",
            r#"serde(default = "default_site")"#,
            r#"serde(rename = "mag")"#
        ]
    );
    let stats = sym(&facts, "stats");
    let stars = call(stats, "db::count_stars");
    assert!((stars.line, stars.awaited) == (89, true));
    let obs = call(stats, "db::count_observations");
    assert_eq!(obs.line, 90);
    assert_eq!(obs.string_args, vec!["SELECT count(*) FROM observations"]);
    assert_eq!(call(stats, "first").line, 92);
    assert_eq!(call(stats, "second").line, 92);
    let recv = call(stats, "rx.recv");
    assert!((recv.line, recv.awaited) == (94, true));
    assert!(call(stats, "handle").awaited);
    assert_eq!(call(stats, "tokio::time::sleep").line, 95);
    assert_eq!(call(stats, "fmt_count").line, 97);
    assert_eq!(call(stats, "names.len").line, 97);
    assert_eq!(call(stats, "println!").string_args, vec!["{} {}"]);
    assert!(!names(stats).contains(&"skipped"));

    let max = sym(&facts, "MAX");
    assert_eq!(max.kind, SymbolKind::Constant);
    assert_eq!(max.returns.as_deref(), Some("usize"));
}

#[test]
fn python_fastapi_route() {
    let facts = PythonAdapter
        .parse("app/api.py", include_str!("fixtures/api.py"), "app.api")
        .unwrap();
    assert_eq!(facts.language, Language::Python);
    let from_db = facts.imports.iter().find(|i| i.path == ".db").unwrap();
    assert_eq!(from_db.names, vec!["get_session as session_dep", "Repo"]);
    assert!(facts.imports.iter().any(|i| i.path == "asyncio"));

    assert_eq!(sym(&facts, "TIMEOUT").kind, SymbolKind::Constant);

    let svc = sym(&facts, "UserService");
    assert_eq!(svc.kind, SymbolKind::Type);
    assert_eq!(svc.implements, vec!["Repo"]);
    assert_eq!(
        svc.doc.as_deref(),
        Some("Reads users from the upstream service.")
    );

    let init = sym(&facts, "UserService.__init__");
    assert_eq!(init.kind, SymbolKind::Method);
    assert_eq!(init.visibility, Visibility::Public);
    assert_eq!(init.params[1].ty.as_deref(), Some("httpx.AsyncClient"));
    assert_eq!(init.params[2].default.as_deref(), Some("3"));

    let fetch_one = sym(&facts, "UserService.fetch_one");
    assert!(fetch_one.is_async);
    assert_eq!(fetch_one.returns.as_deref(), Some("dict"));
    assert_eq!(
        fetch_one.doc.as_deref(),
        Some("Fetch a single user.\n\nRetries on failure.")
    );
    assert_eq!(
        fetch_one.signature,
        "async def fetch_one(self, user_id: int) -> dict"
    );
    let get = call(fetch_one, "self.client.get");
    assert_eq!(get.receiver.as_deref(), Some("self.client"));
    assert_eq!(get.string_args, vec!["/users/{}"]);
    assert!(get.awaited && get.loop_depth == 1);
    let body = fetch_one.body.as_ref().unwrap();
    assert_eq!(body.early_exits, 2);
    assert_eq!(body.cyclomatic, 4);

    assert_eq!(
        sym(&facts, "UserService._normalise").visibility,
        Visibility::Private
    );
    assert_eq!(
        call(sym(&facts, "UserService._normalise"), "k.lower").loop_depth,
        1
    );

    let route = sym(&facts, "list_users");
    assert_eq!(route.attributes, vec![r#"app.get("/users")"#]);
    assert_eq!(
        route.params[1].default.as_deref(),
        Some("Depends(session_dep)")
    );
    assert!(call(route, "asyncio.gather").awaited);
    let inner = call(route, "svc.fetch_one");
    assert!(!inner.awaited && inner.loop_depth == 1);
    assert_eq!(call(route, "svc._normalise").loop_depth, 1);
    assert!(!names(route).contains(&"item.get"));

    assert!(!names(route).contains(&"ids"));

    let legacy = sym(&facts, "legacy");
    let gets: Vec<_> = legacy
        .body
        .as_ref()
        .unwrap()
        .calls
        .iter()
        .filter(|c| c.name == "get")
        .collect();
    assert_eq!(gets[0].string_args, vec!["{}/users/{}"]);
    assert_eq!(gets[1].string_args, vec!["{}/users/{}"]);
    assert_eq!(gets[2].string_args, vec!["{}/health"]);
    assert_eq!(gets[2].line, 57);
    assert_eq!(call(legacy, "httpx.Client").line, 56);

    let handler = sym(&facts, "handler");
    assert_eq!(handler.kind, SymbolKind::Function);
    assert_eq!(handler.signature, "handler = session_dep(list_users)");
    assert_eq!(names(handler), vec!["session_dep", "list_users"]);
    assert!(!facts.symbols.iter().any(|s| s.name == "not_callable"));
    let module = sym(&facts, "<module>");
    assert_eq!(module.kind, SymbolKind::Function);
    assert_eq!((module.span.start_line, module.span.end_line), (1, 66));
    assert_eq!(call(module, "FastAPI").line, 8);
    assert_eq!(
        call(module, "app.add_api_route").string_args,
        vec!["/users"]
    );
    assert_eq!(call(module, "httpx.Client").line, 61);
    assert!(call(module, "uvicorn.run").conditional);
    assert!(!names(module).contains(&"session_dep"));
    assert!(!names(module).contains(&"range"));

    let key = sym(&facts, "list_users.key");
    assert_eq!(key.parent.as_deref(), Some("list_users"));
    assert_eq!(key.kind, SymbolKind::Function);
    assert_eq!(call(key, "item.get").string_args, vec!["name", ""]);
}

#[test]
fn typescript_fetch_client() {
    let facts = TsAdapter(Flavor::TypeScript)
        .parse(
            "src/api/client.ts",
            include_str!("fixtures/client.ts"),
            "src/api/client",
        )
        .unwrap();
    assert_eq!(facts.language, Language::TypeScript);
    assert_eq!(facts.imports[0].names, vec!["writable"]);
    assert_eq!(facts.imports[0].path, "svelte/store");
    assert_eq!(facts.imports[2].alias.as_deref(), Some("log"));

    assert_eq!(sym(&facts, "BASE_URL").visibility, Visibility::Private);
    let repo = sym(&facts, "UserRepo");
    assert_eq!(repo.kind, SymbolKind::Interface);
    assert_eq!(repo.implements, vec!["Base"]);

    let fetch_users = sym(&facts, "fetchUsers");
    assert!(fetch_users.is_async);
    assert_eq!(fetch_users.visibility, Visibility::Public);
    assert_eq!(
        fetch_users.doc.as_deref(),
        Some("Fetches users from the API.\n@param ids user ids")
    );
    assert_eq!(fetch_users.params[1].name, "signal");
    assert_eq!(fetch_users.params[1].ty.as_deref(), Some("AbortSignal"));
    assert_eq!(fetch_users.returns.as_deref(), Some("Promise<User[]>"));
    assert_eq!(
        (fetch_users.span.start_line, fetch_users.span.end_line),
        (15, 25)
    );
    let fetch = call(fetch_users, "fetch");
    assert_eq!(fetch.string_args, vec!["{}/users/{}"]);
    assert!(fetch.awaited && fetch.loop_depth == 1);
    assert!(call(fetch_users, "Error").conditional);
    assert!(call(fetch_users, "res.json").awaited);
    let body = fetch_users.body.as_ref().unwrap();
    assert_eq!(body.early_exits, 1);
    assert_eq!(body.closures, 1);
    assert_eq!(body.branches, 2);

    let save = sym(&facts, "saveUser");
    assert_eq!(save.kind, SymbolKind::Function);
    assert!(save.is_async);
    assert_eq!(save.doc.as_deref(), Some("Debounced save."));
    assert_eq!(save.params[1].default.as_deref(), Some("2"));
    assert_eq!(
        save.signature,
        "export const saveUser = async (user: User, retries = 2): Promise<void>"
    );
    assert_eq!(call(save, "fetch").string_args, vec!["/api/users"]);

    let client = sym(&facts, "UserClient");
    assert_eq!(client.implements, vec!["UserRepo"]);
    assert_eq!(
        client.signature,
        "export default class UserClient implements UserRepo"
    );
    let list = sym(&facts, "UserClient.list");
    assert_eq!(list.kind, SymbolKind::Method);
    assert_eq!(list.doc.as_deref(), Some("List all users."));
    assert!(call(list, "fetchUsers").awaited);
    let set = call(list, "this.cache.set");
    assert!(set.in_closure);
    assert_eq!(set.receiver.as_deref(), Some("this.cache"));
    let get = sym(&facts, "UserClient.get");
    assert_eq!(get.attributes, vec!["memoize()"]);
    assert_eq!(get.body.as_ref().unwrap().branches, 1);

    let legacy = sym(&facts, "legacy");
    let fetches: Vec<_> = legacy
        .body
        .as_ref()
        .unwrap()
        .calls
        .iter()
        .filter(|c| c.callee == "fetch")
        .collect();
    assert_eq!(fetches[0].string_args, vec!["{}/users/{}/posts"]);
    assert!(fetches[1].string_args.is_empty());
    assert!(fetches[2].string_args.is_empty());
    assert_eq!(call(legacy, "client.get").line, 59);
    assert_eq!(call(legacy, "then").line, 60);
    assert_eq!(call(legacy, "r.json").line, 60);

    assert_eq!(sym(&facts, "helper").visibility, Visibility::Private);
    assert_eq!(sym(&facts, "helper").body.as_ref().unwrap().cyclomatic, 2);
}

#[test]
fn svelte_component_script() {
    let facts = SfcAdapter::SVELTE
        .parse(
            "src/lib/Users.svelte",
            include_str!("fixtures/Users.svelte"),
            "src/lib/Users.svelte",
        )
        .unwrap();
    assert_eq!(facts.language, Language::TypeScript);
    assert_eq!(facts.lines, 21);
    assert_eq!(facts.imports[1].path, "$lib/api/client");
    let reload = sym(&facts, "reload");
    assert!(reload.is_async);
    assert_eq!(reload.doc.as_deref(), Some("Reload from the API."));
    assert_eq!((reload.span.start_line, reload.span.end_line), (8, 10));
    let fetch = call(reload, "fetchUsers");
    assert_eq!(fetch.line, 9);
    assert!(fetch.awaited);
    let on_click = sym(&facts, "onClick");
    assert_eq!(on_click.span.start_line, 12);
    assert_eq!(call(on_click, "reload").line, 13);
    assert_eq!(on_click.attributes, vec!["ui:click"]);
    assert_eq!(reload.attributes, vec!["ui:click"]);
}

#[test]
fn typescript_callable_bindings_and_module() {
    let facts = TsAdapter(Flavor::TypeScript)
        .parse("src/main.ts", include_str!("fixtures/main.ts"), "src/main")
        .unwrap();
    let on_launch = sym(&facts, "onLaunch");
    assert_eq!(on_launch.kind, SymbolKind::Function);
    assert!(on_launch.is_async);
    assert_eq!(on_launch.doc.as_deref(), Some("Launch button."));
    assert_eq!(
        on_launch.signature,
        r#"const onLaunch = tracedHandler("click Launch", { functionName: "onLaunch" }, async ()"#
    );
    assert_eq!(
        (on_launch.span.start_line, on_launch.span.end_line),
        (14, 17)
    );
    assert_eq!(
        call(on_launch, "tracedHandler").string_args,
        vec!["click Launch"]
    );
    assert!(!call(on_launch, "tracedHandler").in_closure);
    assert!(call(on_launch, "launchObservation").in_closure);

    let on_list = sym(&facts, "onList");
    assert_eq!(on_list.visibility, Visibility::Public);
    assert_eq!(on_list.attributes, vec!["ui:click"]);
    assert_eq!(names(on_list), vec!["wrap", "show"]);
    assert!(!facts.symbols.iter().any(|s| s.name == "notCallable"));

    let module = sym(&facts, "<module>");
    assert_eq!((module.span.start_line, module.span.end_line), (1, 33));
    assert_eq!(call(module, "initTelemetry").line, 4);
    assert_eq!(call(module, "compute").line, 20);
    assert_eq!(
        call(module, "document.querySelector").string_args,
        vec!["#output"]
    );
    assert!(call(module, "onLaunch").in_closure);
    assert_eq!(call(module, "listStars").loop_depth, 1);
    assert_eq!(
        names(module).iter().filter(|c| **c == "onLaunch").count(),
        2
    );
    assert_eq!(names(module).iter().filter(|c| **c == "onList").count(), 1);
    assert_eq!(on_launch.attributes, vec!["ui:submit"]);
    assert!(!names(module).contains(&"tracedHandler"));
    assert!(!names(module).contains(&"wrap"));
    assert!(!names(module).contains(&"String"));
    assert_eq!(
        facts
            .symbols
            .iter()
            .filter(|s| s.name == "<module>")
            .count(),
        1
    );
}

#[test]
fn vue_template_handlers() {
    let facts = SfcAdapter::VUE
        .parse(
            "src/Counter.vue",
            include_str!("fixtures/Counter.vue"),
            "src/Counter.vue",
        )
        .unwrap();
    assert_eq!(facts.language, Language::TypeScript);
    assert_eq!(sym(&facts, "increment").attributes, vec!["ui:click"]);
    assert_eq!(sym(&facts, "reset").attributes, vec!["ui:click"]);
    assert_eq!(sym(&facts, "increment").span.start_line, 9);
}

#[test]
fn tsx_jsx_handlers() {
    let facts = TsAdapter(Flavor::Tsx)
        .parse("src/App.tsx", include_str!("fixtures/App.tsx"), "src/App")
        .unwrap();
    let app = sym(&facts, "App");
    assert!(app.attributes.is_empty());
    assert_eq!(names(app).iter().filter(|c| **c == "setN").count(), 3);
    assert!(call(app, "Number").in_closure);
    assert_eq!(
        facts
            .symbols
            .iter()
            .map(|s| s.name.as_str())
            .collect::<Vec<_>>(),
        vec!["App"]
    );
}

#[test]
fn tsx_top_level_jsx_handlers() {
    let src = "export const bump = () => 1;\nfunction reset() {}\nexport const View = () => <button onClick={bump} onBlur={() => reset()} />;\n";
    let facts = TsAdapter(Flavor::Tsx)
        .parse("View.tsx", src, "View")
        .unwrap();
    assert_eq!(sym(&facts, "bump").attributes, vec!["ui:click"]);
    assert_eq!(sym(&facts, "reset").attributes, vec!["ui:blur"]);
    assert!(sym(&facts, "View").attributes.is_empty());
}

#[test]
fn go_http_handler() {
    let facts = GoAdapter
        .parse(
            "internal/api/server.go",
            include_str!("fixtures/server.go"),
            "example.com/svc/internal/api",
        )
        .unwrap();
    assert_eq!(facts.language, Language::Go);
    assert_eq!(facts.imports[0].path, "database/sql");
    let logrus = facts
        .imports
        .iter()
        .find(|i| i.path == "github.com/sirupsen/logrus")
        .unwrap();
    assert_eq!(logrus.alias.as_deref(), Some("log"));

    let server = sym(&facts, "Server");
    assert_eq!(server.kind, SymbolKind::Type);
    assert_eq!(server.doc.as_deref(), Some("Server serves users."));
    assert_eq!(server.signature, "type Server struct");
    assert_eq!(sym(&facts, "Handler").kind, SymbolKind::Interface);
    assert_eq!(sym(&facts, "MaxUsers").kind, SymbolKind::Constant);

    let list = sym(&facts, "Server.ListUsers");
    assert_eq!(list.kind, SymbolKind::Method);
    assert_eq!(list.parent.as_deref(), Some("Server"));
    assert_eq!(list.visibility, Visibility::Public);
    assert_eq!(
        list.doc.as_deref(),
        Some("ListUsers fans out one query per id and waits for all of them.\nIt writes JSON.")
    );
    assert_eq!(list.params[0].name, "w");
    assert_eq!(list.params[0].ty.as_deref(), Some("http.ResponseWriter"));
    assert_eq!(list.params[1].ty.as_deref(), Some("*http.Request"));
    let query = call(list, "s.db.QueryRow");
    assert_eq!(query.receiver.as_deref(), Some("s.db"));
    assert_eq!(
        query.string_args,
        vec!["SELECT name FROM users WHERE id = $1"]
    );
    assert!(query.in_closure && query.loop_depth == 1 && !query.conditional);
    assert!(call(list, "log.Error").conditional && call(list, "log.Error").in_closure);
    assert!(!call(list, "wg.Wait").in_closure);
    assert!(call(list, "http.Error").conditional);
    let body = list.body.as_ref().unwrap();
    assert_eq!(body.closures, 1);
    assert_eq!(body.loops, 1);
    assert_eq!(body.branches, 3);
    assert_eq!(body.cyclomatic, 5);
    assert_eq!(body.early_exits, 1);

    let new_server = sym(&facts, "newServer");
    let spawn = call(new_server, "go");
    assert_eq!((spawn.name.as_str(), spawn.line), ("go", 49));
    let warm = call(new_server, "warm");
    assert!(warm.in_closure);
    assert_eq!(warm.line, 49);
    assert!(!call(new_server, "http.HandleFunc").in_closure);
    let handler = call(new_server, "newServer");
    assert_eq!(handler.line, 50);
    assert!(!handler.in_closure);
    assert!(call(sym(&facts, "Server.ListUsers"), "wg.Done").in_closure);

    let ping = sym(&facts, "ping");
    let gets: Vec<_> = ping
        .body
        .as_ref()
        .unwrap()
        .calls
        .iter()
        .filter(|c| c.callee == "http.Get")
        .collect();
    assert_eq!(gets[0].string_args, vec!["{}/users/{}"]);
    assert_eq!(gets[1].string_args, vec!["{}/health"]);
    assert!(names(ping).contains(&"fmt.Sprintf"));
    assert_eq!(call(ping, "client.Get").line, 60);
    assert_eq!(call(ping, "Do").line, 61);
    assert_eq!(new_server.visibility, Visibility::Private);
    assert_eq!(new_server.returns.as_deref(), Some("(*Server, error)"));
    assert_eq!(new_server.params[1].ty.as_deref(), Some("...string"));
    let module = sym(&facts, "<module>");
    assert_eq!(names(module), vec!["newServer", "log.New"]);
    assert_eq!(call(module, "log.New").line, 67);
}

#[test]
fn index_tree_walks_and_resolves_modules() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let write = |rel: &str, content: &str| {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, content).unwrap();
    };
    write(".gitignore", "ignored/\n");
    write("Cargo.toml", "[workspace]\nmembers = [\"crates/*\"]\n");
    write(
        "crates/my-app/Cargo.toml",
        "[package]\nname = \"my-app\"\nversion = \"0.1.0\"\n",
    );
    write(
        "crates/my-app/src/lib.rs",
        "pub mod api;\npub fn root() {}\n",
    );
    write("crates/my-app/src/api/mod.rs", "pub mod sessions;\n");
    write(
        "crates/my-app/src/api/sessions.rs",
        "pub fn list() { crate::root(); }\n",
    );
    write("crates/my-app/src/bin/cli.rs", "fn main() {}\n");
    write("py/src/app/__init__.py", "");
    write("py/src/app/worker/__init__.py", "");
    write("py/src/app/worker/run.py", "def run():\n    pass\n");
    write("scripts/tool.py", "def main():\n    pass\n");
    write("go/go.mod", "module example.com/svc\n\ngo 1.22\n");
    write(
        "go/internal/api/server.go",
        "package api\n\nfunc Serve() {}\n",
    );
    write(
        "web/src/lib/api/client.ts",
        "export const ping = () => fetch('/ping');\n",
    );
    write(
        "web/src/lib/Users.svelte",
        "<script>\n  function f() {}\n</script>\n",
    );
    write("web/src/legacy.js", "function old() {}\n");
    write(
        "web/src/App.tsx",
        "export function App() { return null; }\n",
    );
    write("web/src/vendor.min.js", "function x(){}\n");
    write("web/node_modules/dep/index.js", "module.exports = 1;\n");
    write("target/debug/gen.rs", "fn gen() {}\n");
    write("ignored/skipped.rs", "fn skipped() {}\n");
    write("README.md", "# nope\n");
    write("big.rs", &"fn big() {}\n".repeat(100_000));

    let facts = index_tree(root).unwrap();
    let by_path: std::collections::HashMap<_, _> =
        facts.iter().map(|f| (f.path.as_str(), f)).collect();
    let mut paths: Vec<_> = by_path.keys().copied().collect();
    paths.sort_unstable();
    assert_eq!(
        paths,
        vec![
            "crates/my-app/src/api/mod.rs",
            "crates/my-app/src/api/sessions.rs",
            "crates/my-app/src/bin/cli.rs",
            "crates/my-app/src/lib.rs",
            "go/internal/api/server.go",
            "py/src/app/__init__.py",
            "py/src/app/worker/__init__.py",
            "py/src/app/worker/run.py",
            "scripts/tool.py",
            "web/src/App.tsx",
            "web/src/legacy.js",
            "web/src/lib/Users.svelte",
            "web/src/lib/api/client.ts",
        ]
    );
    assert_eq!(by_path["crates/my-app/src/lib.rs"].module, "my_app");
    assert_eq!(
        by_path["crates/my-app/src/api/mod.rs"].module,
        "my_app::api"
    );
    assert_eq!(
        by_path["crates/my-app/src/api/sessions.rs"].module,
        "my_app::api::sessions"
    );
    assert_eq!(
        by_path["crates/my-app/src/bin/cli.rs"].module,
        "my_app::bin::cli"
    );
    assert_eq!(by_path["py/src/app/worker/run.py"].module, "app.worker.run");
    assert_eq!(
        by_path["py/src/app/worker/__init__.py"].module,
        "app.worker"
    );
    assert_eq!(by_path["scripts/tool.py"].module, "scripts.tool");
    assert_eq!(
        by_path["go/internal/api/server.go"].module,
        "example.com/svc/internal/api"
    );
    assert_eq!(
        by_path["web/src/lib/api/client.ts"].module,
        "web/src/lib/api/client"
    );
    assert_eq!(
        by_path["web/src/lib/Users.svelte"].module,
        "web/src/lib/Users.svelte"
    );
    assert_eq!(by_path["web/src/legacy.js"].language, Language::JavaScript);
    assert_eq!(
        by_path["web/src/lib/Users.svelte"].language,
        Language::JavaScript
    );
    assert_eq!(by_path["web/src/App.tsx"].symbols[0].name, "App");
    assert_eq!(
        by_path["crates/my-app/src/api/sessions.rs"].symbols[0]
            .body
            .as_ref()
            .unwrap()
            .calls[0]
            .callee,
        "crate::root"
    );

    assert_eq!(
        module_path(Path::new("/nonexistent"), "src/main.rs", Language::Rust),
        "crate"
    );
    assert_eq!(
        module_path(
            Path::new("/nonexistent"),
            "src/pkg/mod.py",
            Language::Python
        ),
        "pkg.mod"
    );
}

#[test]
fn typescript_object_methods_fields_and_refs() {
    let facts = TsAdapter(Flavor::TypeScript)
        .parse(
            "src/objects.ts",
            include_str!("fixtures/objects.ts"),
            "src/objects",
        )
        .unwrap();

    let session = sym(&facts, "keys.session");
    assert_eq!(session.kind, SymbolKind::Method);
    assert_eq!(session.parent.as_deref(), Some("keys"));
    assert_eq!(session.visibility, Visibility::Public);
    assert_eq!(sym(&facts, "keys.all").parent.as_deref(), Some("keys"));
    assert!(!facts.symbols.iter().any(|s| s.name == "<module>"));

    let spawn = sym(&facts, "useSessionActions.spawn");
    assert_eq!(spawn.parent.as_deref(), Some("useSessionActions"));
    assert_eq!(call(spawn, "endpoints.spawn").line, 14);
    let archive = sym(&facts, "useSessionActions.archive");
    assert_eq!(
        call(archive, "api.post").string_args,
        vec!["/sessions/{}/archive"]
    );
    let factory = sym(&facts, "useSessionActions");
    assert!(names(factory).contains(&"api.get"));
    assert!(
        !names(factory).contains(&"endpoints.spawn"),
        "{:?}",
        names(factory)
    );
    assert!(!names(factory).contains(&"api.post"));

    let form = sym(&facts, "SpawnForm");
    let fields: Vec<(&str, Option<&str>)> = form
        .params
        .iter()
        .map(|p| (p.name.as_str(), p.ty.as_deref()))
        .collect();
    assert_eq!(
        fields,
        vec![
            ("actions", Some("useSessionActions()")),
            ("store", Some("Store")),
            ("files", Some("File[]")),
            ("client", Some("Client")),
        ]
    );

    let submit = sym(&facts, "SpawnForm.submit");
    let spawn_call = call(submit, "this.actions.spawn");
    assert_eq!(spawn_call.receiver.as_deref(), Some("this.actions"));
    assert!(spawn_call.awaited);
    assert!(names(submit).contains(&"show"), "{:?}", names(submit));
    assert!(!names(submit).contains(&"body"), "{:?}", names(submit));
    assert!(names(submit).contains(&"this.files"));
}

#[test]
fn rust_message_sites() {
    let facts = RustAdapter
        .parse(
            "src/launch.rs",
            include_str!("fixtures/messages.rs"),
            "app::launch",
        )
        .unwrap();
    let launch = sym(&facts, "launch");
    let sent: Vec<(&str, u32, Option<&str>)> = launch
        .body
        .as_ref()
        .unwrap()
        .calls
        .iter()
        .filter(|c| c.callee == "<message>")
        .map(|c| (c.string_args[0].as_str(), c.line, c.receiver.as_deref()))
        .collect();
    assert_eq!(
        sent,
        vec![
            ("FrameDown::Command", 5, Some("bus")),
            ("FrameDown::Reconcile", 6, Some("bus")),
        ]
    );
    let handle = sym(&facts, "handle");
    let matched: Vec<(&str, u32)> = handle
        .body
        .as_ref()
        .unwrap()
        .calls
        .iter()
        .filter(|c| c.callee == "<match>")
        .map(|c| (c.string_args[0].as_str(), c.line))
        .collect();
    assert_eq!(
        matched,
        vec![
            ("FrameDown::Command", 13),
            ("FrameDown::Reconcile", 14),
            ("FrameDown::Ack", 20),
        ]
    );
}
