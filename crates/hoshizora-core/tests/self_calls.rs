use std::path::Path;

use hoshizora_core::{Packs, build_graph};
use hoshizora_model::{Confidence, EdgeKind, Graph};

fn graph() -> Graph {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/constructors");
    let files = hoshizora_adapters::index_tree(&root).unwrap();
    build_graph(&root, files, &Packs::builtin())
}

fn calls(g: &Graph, from: &str) -> Vec<(String, Confidence)> {
    let from = format!("fn:src/lib.rs#{from}");
    let mut out: Vec<(String, Confidence)> = g
        .edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Calls && e.from == from)
        .map(|e| {
            (
                e.to.trim_start_matches("fn:src/lib.rs#").to_owned(),
                e.confidence,
            )
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out.dedup_by(|a, b| a.0 == b.0);
    out
}

fn time(g: &Graph, name: &str) -> String {
    let id = format!("fn:src/lib.rs#{name}");
    let node = g.nodes.iter().find(|n| n.id == id).unwrap();
    node.function.as_ref().unwrap().complexity.time.clone()
}

#[test]
fn constructors_of_other_types_are_not_self_calls() {
    let g = graph();
    assert_eq!(
        calls(&g, "Driver::new"),
        vec![("Inner::new".to_owned(), Confidence::Exact)]
    );
    assert!(calls(&g, "Inner::new").is_empty());
    assert_eq!(time(&g, "Driver::new"), "O(1)");
}

#[test]
fn methods_on_fields_resolve_through_the_field_type() {
    let g = graph();
    assert_eq!(
        calls(&g, "Driver::is_empty"),
        vec![("Inner::is_empty".to_owned(), Confidence::Exact)]
    );
    assert!(calls(&g, "Inner::is_empty").is_empty());
    assert_eq!(time(&g, "Driver::is_empty"), "O(1)");
}

#[test]
fn real_branching_recursion_stays_exponential() {
    let g = graph();
    assert_eq!(calls(&g, "walk").len(), 1);
    assert_eq!(time(&g, "walk"), "O(2^n)");
}

#[test]
fn one_self_call_per_arm_is_linear() {
    let g = graph();
    assert_eq!(time(&g, "total"), "O(n)");
}

#[test]
fn super_paths_do_not_resolve_to_the_caller() {
    let g = graph();
    let self_edges = g
        .edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Calls && e.from == e.to && e.from.ends_with("draw"))
        .count();
    assert_eq!(self_edges, 0);
}

#[test]
fn parameters_shadow_functions_of_the_same_name() {
    let g = graph();
    assert_eq!(
        calls(&g, "part"),
        vec![("is_final".to_owned(), Confidence::Name)]
    );
    assert_eq!(time(&g, "part"), "O(1)");
}
