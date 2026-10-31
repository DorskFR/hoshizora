use std::path::Path;

use hoshizora_core::Index;
use hoshizora_model::api::{LaneKind, Story};
use hoshizora_model::{
    Confidence, Edge, EdgeKind, Effect, EffectKind, Effects, EntrypointKind, FunctionInfo, Graph,
    Language, Node, NodeKind, Visibility,
};

fn node(id: &str, kind: NodeKind, parent: Option<&str>, language: Option<Language>) -> Node {
    let name = id
        .rsplit_once('#')
        .map(|(_, n)| n)
        .or_else(|| id.split_once(':').map(|(_, n)| n))
        .unwrap_or(id)
        .rsplit('.')
        .next()
        .unwrap()
        .to_owned();
    Node {
        id: id.to_owned(),
        kind,
        name,
        qualified_name: id.split_once(':').map(|(_, n)| n).unwrap_or(id).to_owned(),
        language,
        file: id
            .strip_prefix("fn:")
            .and_then(|r| r.split_once('#'))
            .map(|(f, _)| f.to_owned()),
        span: None,
        parent: parent.map(str::to_owned),
        function: None,
        effects: Effects::default(),
        entrypoint: None,
        dead: false,
        lines: 1,
    }
}

fn func(id: &str, parent: &str, language: Language, is_async: bool) -> Node {
    let mut n = node(id, NodeKind::Function, Some(parent), Some(language));
    n.function = Some(FunctionInfo {
        signature: String::new(),
        params: vec![],
        returns: None,
        doc: None,
        is_async,
        is_method: false,
        visibility: Visibility::Public,
        attributes: vec![],
        cyclomatic: 1,
        branches: 0,
        loops: 0,
        max_loop_depth: 0,
        recursive: false,
        complexity: hoshizora_model::Complexity {
            time: "O(1)".into(),
            space: "O(1)".into(),
            basis: vec![],
        },
        fan_in: 0,
        fan_out: 0,
    });
    n
}

fn effect(kind: EffectKind, target: &str) -> Effect {
    Effect {
        kind,
        target: Some(target.to_owned()),
        via: "x".into(),
        line: 1,
        loop_depth: 0,
        awaited: true,
        rule: "test".into(),
    }
}

fn edge(from: &str, to: &str, kind: EdgeKind, confidence: Confidence, line: u32) -> Edge {
    let mut e = Edge::new(from, to, kind, confidence);
    e.line = Some(line);
    e.awaited = true;
    e
}

/// UI fn → helper → client fetch → route → handler → helper → db write + spawn.
fn synthetic() -> Graph {
    let ts = Language::TypeScript;
    let rs = Language::Rust;
    let mut submit = func("fn:web/src/spawnForm.ts#Form.submit", "mod:web", ts, true);
    submit.entrypoint = Some(EntrypointKind::UiHandler);
    submit.parent = Some("type:web/src/spawnForm.ts#Form".into());
    let mut client = func("fn:web/src/api.ts#postSpawn", "mod:web", ts, true);
    client.effects.direct = vec![effect(EffectKind::Network, "/sessions/spawn")];
    let mut handler = func("fn:api/src/spawn.rs#spawn_session", "mod:api", rs, true);
    handler.entrypoint = Some(EntrypointKind::HttpHandler);
    let mut insert = func("fn:api/src/db.rs#insert_session", "mod:api", rs, true);
    insert.effects.direct = vec![effect(EffectKind::DbWrite, "sessions")];
    let mut test_fn = func("fn:api/src/spawn.rs#spawns_a_session", "mod:api", rs, true);
    test_fn.entrypoint = Some(EntrypointKind::Test);
    let mut form_type = node(
        "type:web/src/spawnForm.ts#Form",
        NodeKind::Type,
        Some("mod:web"),
        Some(ts),
    );
    form_type.name = "Form".into();
    Graph {
        nodes: vec![
            node("mod:web", NodeKind::Package, None, Some(ts)),
            node("mod:api", NodeKind::Package, None, Some(rs)),
            form_type,
            submit,
            func("fn:web/src/spawnForm.ts#buildBody", "mod:web", ts, false),
            client,
            node("route:POST /sessions/spawn", NodeKind::Route, None, None),
            handler,
            func("fn:api/src/spawn.rs#validate", "mod:api", rs, false),
            insert,
            func("fn:api/src/spawn.rs#notify_daemon", "mod:api", rs, true),
            node("table:sessions", NodeKind::Table, None, None),
            test_fn,
        ],
        edges: vec![
            edge(
                "fn:web/src/spawnForm.ts#Form.submit",
                "fn:web/src/spawnForm.ts#buildBody",
                EdgeKind::Calls,
                Confidence::Exact,
                10,
            ),
            edge(
                "fn:web/src/spawnForm.ts#Form.submit",
                "fn:web/src/api.ts#postSpawn",
                EdgeKind::Calls,
                Confidence::Exact,
                11,
            ),
            edge(
                "fn:web/src/api.ts#postSpawn",
                "route:POST /sessions/spawn",
                EdgeKind::Requests,
                Confidence::Heuristic,
                5,
            ),
            edge(
                "route:POST /sessions/spawn",
                "fn:api/src/spawn.rs#spawn_session",
                EdgeKind::Handles,
                Confidence::Name,
                0,
            ),
            edge(
                "fn:api/src/spawn.rs#spawn_session",
                "fn:api/src/spawn.rs#validate",
                EdgeKind::Calls,
                Confidence::Name,
                20,
            ),
            edge(
                "fn:api/src/spawn.rs#validate",
                "fn:api/src/db.rs#insert_session",
                EdgeKind::Calls,
                Confidence::Exact,
                30,
            ),
            edge(
                "fn:api/src/db.rs#insert_session",
                "table:sessions",
                EdgeKind::Writes,
                Confidence::Heuristic,
                31,
            ),
            edge(
                "fn:api/src/spawn.rs#spawn_session",
                "fn:api/src/spawn.rs#notify_daemon",
                EdgeKind::Spawns,
                Confidence::Exact,
                22,
            ),
            edge(
                "fn:api/src/spawn.rs#spawn_session",
                "fn:api/src/spawn.rs#spawns_a_session",
                EdgeKind::Calls,
                Confidence::Name,
                23,
            ),
        ],
        ..Graph::default()
    }
}

fn print(story: &Story) -> String {
    let mut out = String::new();
    out.push_str(&format!("root {}\n", story.root));
    for l in &story.lanes {
        out.push_str(&format!("lane {:?} {} ({})\n", l.kind, l.label, l.id));
    }
    for s in &story.steps {
        let effects: Vec<String> = s
            .effects
            .iter()
            .map(|e| format!("{:?}:{}", e.kind, e.target.clone().unwrap_or_default()))
            .collect();
        out.push_str(&format!(
            "{:>3} [{}] {:?} {}{}{}\n",
            s.seq,
            story
                .lanes
                .iter()
                .find(|l| l.id == s.lane)
                .map(|l| l.label.as_str())
                .unwrap_or(&s.lane),
            s.kind,
            s.label,
            if effects.is_empty() {
                String::new()
            } else {
                format!("  effects={}", effects.join(","))
            },
            if s.folded.is_empty() {
                String::new()
            } else {
                format!("  folded={}", s.folded.len())
            },
        ));
        for f in &s.folded {
            out.push_str(&format!("        + {f}\n"));
        }
    }
    for l in &story.links {
        out.push_str(&format!(
            "link {:>3} {:?} {} -> {}{}{}\n",
            l.seq,
            l.kind,
            short(&l.from),
            short(&l.to),
            l.label
                .as_ref()
                .map(|s| format!("  [{s}]"))
                .unwrap_or_default(),
            if l.detached { "  detached" } else { "" },
        ));
    }
    if story.truncated {
        out.push_str("truncated\n");
    }
    out
}

fn short(id: &str) -> &str {
    id.rsplit_once('#').map(|(_, n)| n).unwrap_or(id)
}

#[test]
fn folds_helpers_and_orders_lanes() {
    let index = Index::new(synthetic());
    let story = index
        .story("fn:web/src/spawnForm.ts#Form.submit", 10)
        .unwrap();
    let text = print(&story);
    let labels: Vec<&str> = story.steps.iter().map(|s| s.label.as_str()).collect();
    assert_eq!(
        labels,
        [
            "Form.submit",
            "postSpawn",
            "POST /sessions/spawn",
            "spawn_session",
            "notify_daemon",
            "insert_session",
            "sessions"
        ],
        "{text}"
    );
    let kinds: Vec<LaneKind> = story.lanes.iter().map(|l| l.kind).collect();
    assert_eq!(
        kinds,
        [LaneKind::Ui, LaneKind::Service, LaneKind::Database],
        "{text}"
    );
    assert_eq!(story.lanes[0].label, "web");
    assert_eq!(story.lanes[1].label, "api");
    assert_eq!(story.steps[0].folded, ["fn:web/src/spawnForm.ts#buildBody"]);
    assert_eq!(story.steps[3].folded, ["fn:api/src/spawn.rs#validate"]);
    assert!(!text.contains("spawns_a_session"), "{text}");
    assert_eq!(story.steps[2].lane, "mod:api");
    let label = |from: &str, to: &str| {
        story
            .links
            .iter()
            .find(|l| l.from.ends_with(from) && l.to.ends_with(to))
            .unwrap_or_else(|| panic!("no link {from} -> {to}\n{text}"))
    };
    assert_eq!(
        label("postSpawn", "route:POST /sessions/spawn")
            .label
            .as_deref(),
        Some("POST /sessions/spawn")
    );
    assert_eq!(
        label("route:POST /sessions/spawn", "spawn_session")
            .label
            .as_deref(),
        Some("POST /sessions/spawn")
    );
    assert_eq!(
        label("spawn_session", "insert_session").kind,
        EdgeKind::Calls
    );
    assert_eq!(
        label("insert_session", "table:sessions").label.as_deref(),
        Some("write sessions")
    );
    let spawn = label("spawn_session", "notify_daemon");
    assert_eq!(spawn.label.as_deref(), Some("spawn"));
    assert!(spawn.detached);
    assert!(!story.truncated);
}

#[test]
fn search_is_separator_insensitive_and_ranks_tests_last() {
    let index = Index::new(synthetic());
    for q in [
        "spawn_session",
        "SpawnSession",
        "spawn session",
        "spawnsession",
    ] {
        let hits = index.search(q, 5);
        assert_eq!(
            hits[0].node, "fn:api/src/spawn.rs#spawn_session",
            "{q}: {hits:?}"
        );
    }
    let hits = index.search("SpawnSession", 10);
    assert!(
        hits.iter().any(|h| h.node == "route:POST /sessions/spawn"),
        "route found by tokens: {hits:?}"
    );
    let hits = index.search("spawn", 10);
    let pos = |id: &str| hits.iter().position(|h| h.node == id).unwrap_or(usize::MAX);
    assert!(pos("route:POST /sessions/spawn") < pos("fn:api/src/spawn.rs#spawns_a_session"));
    assert!(pos("fn:api/src/spawn.rs#spawn_session") < pos("fn:api/src/spawn.rs#spawns_a_session"));
    assert!(pos("fn:web/src/api.ts#postSpawn") < pos("fn:api/src/spawn.rs#spawns_a_session"));
    assert!(
        hits.iter()
            .any(|h| h.node == "fn:web/src/spawnForm.ts#Form.submit"),
        "found via file/type name: {hits:?}"
    );
}

/// `HOSHIZORA_STORY_GRAPH=tmp/x.json HOSHIZORA_STORY_FROM=<id> cargo test -p hoshizora-core --test story real_graph -- --ignored --nocapture`
#[test]
#[ignore]
fn real_graph() {
    let graph = std::env::var("HOSHIZORA_STORY_GRAPH").unwrap();
    let from = std::env::var("HOSHIZORA_STORY_FROM").unwrap();
    let index = Index::new(hoshizora_core::load(Path::new(&graph)).unwrap());
    let story = index.story(&from, 12).expect("node not found");
    println!("{}", print(&story));
}
