use std::path::Path;

use hoshizora_core::{Index, Packs, build_graph};
use hoshizora_model::Insight;

fn index() -> Index {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/precision");
    let files = hoshizora_adapters::index_tree(&root).unwrap();
    Index::new(build_graph(&root, files, &Packs::builtin()))
}

fn io_in_loop(index: &Index, node: &str) -> Vec<String> {
    index
        .static_insights()
        .into_iter()
        .filter_map(|i| match i {
            Insight::IoInLoop {
                node: n, effect, ..
            } if n == node => Some(effect),
            _ => None,
        })
        .collect()
}

fn inefficient(index: &Index, node: &str) -> Option<(Option<u64>, String, Vec<String>)> {
    index.static_insights().into_iter().find_map(|i| match i {
        Insight::Inefficient {
            node: n,
            self_p95_nanos,
            complexity,
            reasons,
        } if n == node => Some((self_p95_nanos, complexity, reasons)),
        _ => None,
    })
}

fn time(index: &Index, node: &str) -> String {
    index
        .node(node)
        .and_then(|n| n.function.as_ref())
        .map(|f| f.complexity.time.clone())
        .unwrap_or_else(|| panic!("{node}"))
}

#[test]
fn method_on_unknown_receiver_bound_by_name_is_not_io_in_loop() {
    let idx = index();
    assert_eq!(
        io_in_loop(&idx, "fn:src/parse.rs#strip_tail"),
        Vec::<String>::new()
    );
    assert_eq!(
        io_in_loop(&idx, "fn:src/store.rs#lookup"),
        Vec::<String>::new()
    );
    assert_eq!(
        io_in_loop(&idx, "fn:src/store.rs#relay"),
        Vec::<String>::new()
    );
}

#[test]
fn bare_identifiers_never_bind_to_methods() {
    let idx = index();
    let targets: Vec<(&str, Option<&str>)> = idx
        .outgoing("fn:src/parse.rs#strip_tail")
        .into_iter()
        .filter(|e| e.kind == hoshizora_model::EdgeKind::Calls)
        .map(|e| (e.to.as_str(), e.attrs.get("receiver").map(String::as_str)))
        .collect();
    assert_eq!(targets, vec![("fn:src/parse.rs#offset", None)]);
}

#[test]
fn io_through_typed_receivers_and_free_functions_stays() {
    let idx = index();
    assert_eq!(
        io_in_loop(&idx, "fn:src/repo.rs#Repo::find_all"),
        vec!["db_read via find"]
    );
    assert_eq!(
        io_in_loop(&idx, "fn:src/repo.rs#warm"),
        vec!["db_read via find"]
    );
    assert_eq!(
        io_in_loop(&idx, "fn:src/repo.rs#sync"),
        vec!["db_write via to_id"]
    );
}

#[test]
fn field_chains_resolve_through_each_fields_type() {
    let idx = index();
    let edge = idx
        .graph()
        .edges
        .iter()
        .find(|e| e.from == "fn:src/repo.rs#to_id" && e.to == "fn:src/repo.rs#IdService::assign")
        .unwrap();
    assert_eq!(edge.confidence, hoshizora_model::Confidence::Exact);
    assert_eq!(
        edge.attrs.get("receiver_type").map(String::as_str),
        Some("IdService")
    );
}

#[test]
fn complexity_only_grows_through_trusted_calls_inside_loops() {
    let idx = index();
    assert_eq!(time(&idx, "fn:src/runner.rs#Runner::dispatch"), "O(1)");
    assert_eq!(time(&idx, "fn:src/runner.rs#Runner::repeat"), "O(n^3)");
    assert_eq!(
        time(&idx, "fn:src/runner.rs#Runner::repeat_unknown"),
        "O(n)"
    );
    assert_eq!(inefficient(&idx, "fn:src/runner.rs#Runner::dispatch"), None);
    let basis = &idx
        .node("fn:src/runner.rs#Runner::repeat")
        .unwrap()
        .function
        .as_ref()
        .unwrap()
        .complexity
        .basis;
    assert!(
        basis
            .iter()
            .any(|r| r == "calls quad (O(n^2)) inside a loop"),
        "{basis:?}"
    );
    let (p95, complexity, _) = inefficient(&idx, "fn:src/runner.rs#Other::heavy").unwrap();
    assert_eq!(p95, None);
    assert_eq!(complexity, "O(n^3)");
}
