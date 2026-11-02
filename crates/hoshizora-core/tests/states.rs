use std::path::Path;

use hoshizora_core::{Index, Packs, build_graph};
use hoshizora_model::{DefaultKind, Graph, Insight, StateSite};

fn graph() -> Graph {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/states");
    let files = hoshizora_adapters::index_tree(&root).unwrap();
    build_graph(&root, files, &Packs::builtin())
}

fn site<'a>(g: &'a Graph, function: &str) -> &'a StateSite {
    let sites: Vec<&StateSite> = g
        .states
        .sites
        .iter()
        .filter(|s| s.function == function)
        .collect();
    assert_eq!(sites.len(), 1, "{function}: {sites:?}");
    sites[0]
}

#[test]
fn domains_are_collected_and_sorted() {
    let g = graph();
    let ids: Vec<&str> = g.states.domains.iter().map(|d| d.id.as_str()).collect();
    assert_eq!(
        ids,
        vec![
            "domain:api/src/lib.rs#Status",
            "domain:src/status.ts#Kind",
            "domain:src/status.ts#Priority",
            "domain:src/status.ts#Status",
        ]
    );
    let ts_status = &g.states.domains[3];
    let values: Vec<&str> = ts_status.values.iter().map(|v| v.name.as_str()).collect();
    assert_eq!(values, vec!["active", "paused", "done", "archived"]);
}

#[test]
fn literal_switch_links_by_value_overlap_within_the_language_family() {
    let g = graph();
    let s = site(&g, "fn:src/jobs/render.ts#badge");
    assert_eq!(s.domain, "domain:src/status.ts#Status");
    assert_eq!(s.subject, "status");
    assert_eq!(s.handled, vec!["active", "paused"]);
    assert_eq!(s.missing, vec!["done", "archived"]);
    assert_eq!(s.default, DefaultKind::ReturnsEmpty);
}

#[test]
fn qualified_switch_links_by_name_and_is_complete() {
    let g = graph();
    let s = site(&g, "fn:src/jobs/render.ts#weight");
    assert_eq!(s.domain, "domain:src/status.ts#Priority");
    assert_eq!(s.handled, vec!["Low", "High"]);
    assert!(s.missing.is_empty());
    assert_eq!(s.default, DefaultKind::Throws);
}

#[test]
fn if_chain_on_as_const_map_links_through_member_names() {
    let g = graph();
    let s = site(&g, "fn:src/jobs/render.ts#icon");
    assert_eq!(s.domain, "domain:src/status.ts#Kind");
    assert_eq!(s.handled, vec!["Note", "Task"]);
    assert_eq!(s.missing, vec!["Event"]);
    assert_eq!(s.default, DefaultKind::ReturnsEmpty);
}

#[test]
fn full_if_chain_has_nothing_missing() {
    let g = graph();
    let s = site(&g, "fn:src/jobs/render.ts#full");
    assert_eq!(s.domain, "domain:src/status.ts#Status");
    assert!(s.missing.is_empty());
    assert_eq!(s.default, DefaultKind::Other);
}

#[test]
fn values_outside_every_domain_stay_unlinked() {
    let g = graph();
    assert!(
        g.states
            .sites
            .iter()
            .all(|s| s.function != "fn:src/jobs/render.ts#twoLetters")
    );
}

#[test]
fn rust_match_links_to_the_rust_enum_not_the_typescript_union() {
    let g = graph();
    let s = site(&g, "fn:api/src/lib.rs#label");
    assert_eq!(s.domain, "domain:api/src/lib.rs#Status");
    assert_eq!(s.missing, vec!["Done"]);
    assert_eq!(s.default, DefaultKind::ReturnsEmpty);
    let s = site(&g, "fn:api/src/lib.rs#closed");
    assert!(s.missing.is_empty());
    assert_eq!(s.default, DefaultKind::None);
}

#[test]
fn states_query_groups_sites_per_domain_and_filters_by_name() {
    let index = Index::new(graph());
    let all = index.states(None);
    assert_eq!(all.domains.len(), 4);
    let only = index.states(Some("Status"));
    let names: Vec<(&str, usize)> = only
        .domains
        .iter()
        .map(|d| (d.domain.file.as_str(), d.sites.len()))
        .collect();
    assert_eq!(names, vec![("api/src/lib.rs", 4), ("src/status.ts", 2)]);
    assert!(index.states(Some("Nope")).domains.is_empty());
}

#[test]
fn silent_defaults_with_missing_values_are_insights() {
    let index = Index::new(graph());
    let mut found: Vec<(String, String, Vec<String>, DefaultKind)> = index
        .static_insights()
        .into_iter()
        .filter_map(|i| match i {
            Insight::UnhandledValues {
                node,
                domain,
                missing,
                default,
                ..
            } => Some((node, domain, missing, default)),
            _ => None,
        })
        .collect();
    found.sort();
    assert_eq!(
        found,
        vec![
            (
                "fn:api/src/lib.rs#label".to_owned(),
                "Status".to_owned(),
                vec!["Done".to_owned()],
                DefaultKind::ReturnsEmpty
            ),
            (
                "fn:src/jobs/render.ts#badge".to_owned(),
                "Status".to_owned(),
                vec!["done".to_owned(), "archived".to_owned()],
                DefaultKind::ReturnsEmpty
            ),
            (
                "fn:src/jobs/render.ts#icon".to_owned(),
                "Kind".to_owned(),
                vec!["Event".to_owned()],
                DefaultKind::ReturnsEmpty
            ),
        ]
    );
}

#[test]
fn projections_and_tests_are_sites_but_not_insights() {
    let g = graph();
    let s = site(&g, "fn:api/src/lib.rs#only_active");
    assert_eq!(s.missing, vec!["Paused", "Done"]);
    assert_eq!(s.default, DefaultKind::ReturnsEmpty);
    let t = site(&g, "fn:api/src/lib.rs#label_in_test");
    assert_eq!(t.missing, vec!["Done"]);
    let index = Index::new(g);
    assert!(index.static_insights().iter().all(|i| !matches!(
        i,
        Insight::UnhandledValues { node, .. }
            if node == "fn:api/src/lib.rs#only_active"
                || node == "fn:api/src/lib.rs#label_in_test"
    )));
}

#[test]
fn graph_json_round_trips_states() {
    let g = graph();
    let json = serde_json::to_string(&g).unwrap();
    let back: Graph = serde_json::from_str(&json).unwrap();
    assert_eq!(back.states, g.states);
    assert_eq!(serde_json::to_string(&back).unwrap(), json);
}
