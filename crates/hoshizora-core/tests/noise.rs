use std::path::Path;

use hoshizora_core::{Index, InsightOptions, Packs, build_graph};
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
            "fn:src/upsert.rs#decay",
            "fn:web/users.ts#fetchAll"
        ]
    );
    let with_tests = idx.static_insights_with(InsightOptions {
        include_tests: true,
        ..InsightOptions::default()
    });
    let all = io_nodes(&with_tests);
    for node in [
        "fn:src/repo.rs#seed",
        "fn:src/repo.rs#loads_every_user",
        "fn:tests/it.rs#reload",
        "fn:web/users.test.ts#fetchAll",
        "fn:conftest.py#warm",
        "fn:src/fixtures/mod.rs#seed_counters",
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
        ("fn:src/loops.rs#per_group", "O(n)"),
        ("fn:src/loops.rs#against_all", "O(n^3)"),
        ("fn:loops.py#cutoff", "O(n)"),
        ("fn:loops.py#around", "O(1)"),
        ("fn:loops.py#capped", "O(n^2)"),
        ("fn:loops.py#lines_of", "O(n)"),
        ("fn:web/tree.ts#height", "O(n)"),
        ("fn:web/tree.ts#fib", "O(2^n)"),
        ("fn:web/tree.ts#condition", "O(1)"),
    ] {
        assert_eq!(time(&idx, node), want, "{node}");
    }
}

#[test]
fn locally_bound_small_or_per_item_loops_do_not_multiply() {
    let idx = index();
    for (node, want) in [
        ("fn:loops.py#near_matches", "O(n^2)"),
        ("fn:web/loops.ts#nearHits", "O(n^2)"),
        ("fn:loops.py#pair_legs", "O(n^2)"),
        ("fn:loops.py#ring_hits", "O(n^2)"),
        ("fn:src/loops.rs#neighbours", "O(n)"),
        ("fn:src/loops.rs#stencil", "O(n)"),
        ("fn:loops.py#by_role", "O(n)"),
        ("fn:loops.py#run_edges", "O(n)"),
        ("fn:web/loops.ts#mixdown", "O(n)"),
        ("fn:loops.py#later_pairs", "O(n^2)"),
        ("fn:loops.py#against_seen", "O(n^2)"),
    ] {
        assert_eq!(time(&idx, node), want, "{node}");
    }
    for node in ["fn:loops.py#halves", "fn:src/loops.rs#split_halves"] {
        assert_ne!(time(&idx, node), "O(2^n)", "{node}");
    }
}

fn inefficient_nodes(insights: &[Insight]) -> Vec<&str> {
    insights
        .iter()
        .filter_map(|i| match i {
            Insight::Inefficient { node, .. } => Some(node.as_str()),
            _ => None,
        })
        .collect()
}

#[test]
fn deep_or_pairwise_nesting_is_inefficient() {
    let idx = index();
    let insights = idx.static_insights();
    let nodes = inefficient_nodes(&insights);
    for node in [
        "fn:src/loops.rs#triples",
        "fn:src/loops.rs#pairs",
        "fn:web/loops.ts#interleave",
        "fn:loops.py#overlaps",
    ] {
        assert!(nodes.contains(&node), "{node} missing: {nodes:?}");
    }
    for node in [
        "fn:src/loops.rs#cross",
        "fn:src/loops.rs#against_all",
        "fn:src/loops.rs#walk",
        "fn:web/loops.ts#publicDrops",
        "fn:loops.py#nearest_free",
        "fn:loops.py#seasons",
    ] {
        assert!(!nodes.contains(&node), "{node} reported: {nodes:?}");
    }
}

#[test]
fn arguments_and_module_aliases_carry_no_effects() {
    let idx = index();
    let insights = idx.static_insights();
    let io = io_nodes(&insights);
    for node in ["fn:web/review.ts#review", "fn:geom.py#normals"] {
        assert!(!io.contains(&node), "{node} reported: {io:?}");
    }
    let rate = idx.outgoing("fn:web/review.ts#rate");
    assert!(
        rate.iter()
            .any(|e| e.to == "fn:web/modes.ts#mode" && e.attrs.contains_key("ref")),
        "{rate:?}"
    );
    assert!(
        !idx.outgoing("fn:geom.py#normals")
            .iter()
            .any(|e| e.to.ends_with("Runner.cross")),
    );
}

#[test]
fn spawned_workers_are_not_io_in_loop() {
    let idx = index();
    let insights = idx.static_insights();
    let io = io_nodes(&insights);
    assert!(!io.contains(&"fn:src/workers.rs#start"), "{io:?}");
}

#[test]
fn awaited_and_spawned_calls_are_not_blocking() {
    let idx = index();
    let insights = idx.static_insights();
    for node in ["fn:src/workers.rs#reap", "fn:src/workers.rs#reap_later"] {
        assert_eq!(
            blocking_reasons(&insights, node),
            Vec::<String>::new(),
            "{node}"
        );
    }
    for node in [
        "fn:src/workers.rs#reap_sync",
        "fn:src/workers.rs#wait_inline",
    ] {
        assert_eq!(blocking_reasons(&insights, node).len(), 1, "{node}");
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
    for node in [
        "fn:src/offload.rs#offloaded",
        "fn:src/offload.rs#same_line",
        "fn:src/offload.rs#hash_off_thread",
        "fn:src/offload.rs#boot_ticker",
    ] {
        assert_eq!(
            blocking_reasons(&insights, node),
            Vec::<String>::new(),
            "{node}"
        );
    }
    for node in [
        "fn:src/offload.rs#inline",
        "fn:src/offload.rs#hash_inline",
        "fn:src/offload.rs#boot_nap",
    ] {
        assert_eq!(blocking_reasons(&insights, node).len(), 1, "{node}");
    }
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
    for node in ["store", "reload", "resync", "localOnly", "withLocal"] {
        let node = format!("fn:web/order.ts#{node}");
        assert!(!par.contains(&node), "{node}: {par:?}");
    }
    assert!(par.iter().any(|n| n == "fn:web/order.ts#both"), "{par:?}");
    assert!(
        par.iter().any(|n| n == "fn:src/workers.rs#sequential"),
        "{par:?}"
    );
    for node in ["joined", "mark_then_count", "two_writes"] {
        let node = format!("fn:src/workers.rs#{node}");
        assert!(!par.contains(&node), "{node}: {par:?}");
    }
}

#[test]
fn read_then_write_needs_the_read_in_the_write() {
    let idx = index();
    let mut races: Vec<String> = idx
        .static_insights()
        .into_iter()
        .filter_map(|i| match i {
            Insight::ReadThenWrite { node, .. } => Some(node),
            _ => None,
        })
        .collect();
    races.sort();
    assert_eq!(
        races,
        [
            "fn:src/upsert.rs#bump",
            "fn:src/upsert.rs#decay",
            "fn:src/upsert.rs#rename"
        ]
    );
}

#[test]
fn deliberate_catch_alls_are_not_unhandled_values() {
    let idx = index();
    let sites: Vec<String> = idx
        .static_insights_with(InsightOptions {
            unhandled_values: true,
            ..InsightOptions::default()
        })
        .into_iter()
        .filter_map(|i| match i {
            Insight::UnhandledValues { node, .. } => Some(node),
            _ => None,
        })
        .collect();
    assert_eq!(sites, ["fn:web/filters.ts#badge"]);
}
