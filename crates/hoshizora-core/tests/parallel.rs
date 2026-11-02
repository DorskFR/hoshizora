use std::path::Path;

use hoshizora_core::{Index, Packs, build_graph};
use hoshizora_model::Insight;

fn flagged() -> Vec<(String, Vec<String>)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/parallel");
    let files = hoshizora_adapters::index_tree(&root).unwrap();
    Index::new(build_graph(&root, files, &Packs::builtin()))
        .static_insights()
        .into_iter()
        .filter_map(|i| match i {
            Insight::Parallelisable { node, children, .. } => Some((node, children)),
            _ => None,
        })
        .collect()
}

#[test]
fn independent_awaits_are_flagged() {
    let found = flagged();
    let rust = found
        .iter()
        .find(|(n, _)| n == "fn:api/src/lib.rs#independent")
        .unwrap_or_else(|| panic!("{found:?}"));
    assert_eq!(rust.1, vec!["fetch_ids:82", "fetch_weather:83"]);
    let ts = found
        .iter()
        .find(|(n, _)| n == "fn:web/src/api.ts#independent")
        .unwrap_or_else(|| panic!("{found:?}"));
    assert_eq!(ts.1, vec!["fetchIds:50", "fetchWeather:51"]);
}

#[test]
fn ordered_awaits_are_not_flagged() {
    let found = flagged();
    let nodes: Vec<&str> = found.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(
        nodes,
        vec![
            "fn:api/src/lib.rs#independent",
            "fn:web/src/api.ts#independent"
        ],
        "{found:?}"
    );
}
