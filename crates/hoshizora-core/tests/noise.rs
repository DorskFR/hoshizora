use std::path::Path;

use hoshizora_core::{Index, Packs, build_graph};
use hoshizora_model::Insight;

fn index() -> Index {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/noise");
    let files = hoshizora_adapters::index_tree(&root).unwrap();
    Index::new(build_graph(&root, files, &Packs::builtin()))
}

fn io_nodes(insights: &[Insight]) -> Vec<&str> {
    let mut nodes: Vec<&str> = insights
        .iter()
        .filter_map(|i| match i {
            Insight::IoInLoop { node, .. } => Some(node.as_str()),
            _ => None,
        })
        .collect();
    nodes.sort_unstable();
    nodes.dedup();
    nodes
}

#[test]
fn test_code_is_left_out_of_insights_unless_asked() {
    let idx = index();
    assert_eq!(
        io_nodes(&idx.static_insights()),
        [
            "fn:loops.py#crawl",
            "fn:src/loops.rs#fetch_each",
            "fn:src/repo.rs#load_all",
            "fn:web/users.ts#fetchAll"
        ]
    );
    let with_tests = idx.static_insights_with(true);
    let all = io_nodes(&with_tests);
    for node in [
        "fn:src/repo.rs#seed",
        "fn:src/repo.rs#loads_every_user",
        "fn:tests/it.rs#reload",
        "fn:web/users.test.ts#fetchAll",
        "fn:conftest.py#warm",
    ] {
        assert!(all.contains(&node), "{node} missing from {all:?}");
    }
}

fn time(index: &Index, node: &str) -> String {
    index
        .node(node)
        .and_then(|n| n.function.as_ref())
        .map(|f| f.complexity.time.clone())
        .unwrap_or_else(|| panic!("{node}"))
}

#[test]
fn bounded_paced_and_event_loops_are_not_io_in_loop() {
    let idx = index();
    let insights = idx.static_insights();
    let io = io_nodes(&insights);
    for node in [
        "fn:src/loops.rs#retry_fetch",
        "fn:src/loops.rs#poll",
        "fn:src/loops.rs#worker",
        "fn:src/loops.rs#purge",
        "fn:loops.py#fetch_status",
    ] {
        assert!(!io.contains(&node), "{node} reported: {io:?}");
    }
    for node in ["fn:src/loops.rs#fetch_each", "fn:loops.py#crawl"] {
        assert!(io.contains(&node), "{node} missing: {io:?}");
    }
}

#[test]
fn loops_over_parts_or_constants_do_not_multiply() {
    let idx = index();
    for (node, want) in [
        ("fn:src/loops.rs#walk", "O(n)"),
        ("fn:src/loops.rs#scores", "O(n)"),
        ("fn:src/loops.rs#pairs", "O(n^2)"),
        ("fn:src/loops.rs#inbox_files", "O(n)"),
        ("fn:src/loops.rs#cross", "O(n^2)"),
        ("fn:loops.py#cells", "O(n)"),
        ("fn:loops.py#all_tips", "O(n)"),
        ("fn:web/loops.ts#publicDrops", "O(n)"),
        ("fn:web/loops.ts#interleave", "O(n^2)"),
    ] {
        assert_eq!(time(&idx, node), want, "{node}");
    }
}

fn blocking_reasons(insights: &[Insight], node: &str) -> Vec<String> {
    insights
        .iter()
        .filter_map(|i| match i {
            Insight::Inefficient {
                node: n, reasons, ..
            } if n == node => Some(reasons.clone()),
            _ => None,
        })
        .flatten()
        .filter(|r| r.contains("blocking"))
        .collect()
}

#[test]
fn work_handed_to_a_spawn_blocking_wrapper_is_not_blocking() {
    let idx = index();
    let insights = idx.static_insights();
    for node in ["fn:src/offload.rs#offloaded", "fn:src/offload.rs#same_line"] {
        assert_eq!(blocking_reasons(&insights, node), Vec::<String>::new());
    }
    assert_eq!(
        blocking_reasons(&insights, "fn:src/offload.rs#inline").len(),
        1,
        "{insights:?}"
    );
}

#[test]
fn calls_after_a_write_are_not_parallelisable() {
    let idx = index();
    let par: Vec<String> = idx
        .static_insights()
        .into_iter()
        .filter_map(|i| match i {
            Insight::Parallelisable { node, .. } => Some(node),
            _ => None,
        })
        .collect();
    for node in ["store", "reload", "resync"] {
        let node = format!("fn:web/order.ts#{node}");
        assert!(!par.contains(&node), "{node}: {par:?}");
    }
    assert!(par.iter().any(|n| n == "fn:web/order.ts#both"), "{par:?}");
}

#[test]
fn conflict_guarded_writes_are_not_read_then_write() {
    let idx = index();
    let races: Vec<String> = idx
        .static_insights()
        .into_iter()
        .filter_map(|i| match i {
            Insight::ReadThenWrite { node, .. } => Some(node),
            _ => None,
        })
        .collect();
    assert_eq!(races, ["fn:src/upsert.rs#rename"]);
}

#[test]
fn deliberate_catch_alls_are_not_unhandled_values() {
    let idx = index();
    let sites: Vec<String> = idx
        .static_insights()
        .into_iter()
        .filter_map(|i| match i {
            Insight::UnhandledValues { node, .. } => Some(node),
            _ => None,
        })
        .collect();
    assert_eq!(sites, ["fn:web/filters.ts#badge"]);
}
