use std::path::Path;

use hoshizora_core::{Index, Packs, build_graph};
use hoshizora_model::{EffectKind, Insight};

fn index() -> Index {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/races");
    let files = hoshizora_adapters::index_tree(&root).unwrap();
    Index::new(build_graph(&root, files, &Packs::builtin()))
}

fn findings(index: &Index) -> Vec<(String, String, u32, u32, String)> {
    index
        .static_insights()
        .into_iter()
        .filter_map(|i| match i {
            Insight::ReadThenWrite {
                node,
                table,
                read_line,
                write_line,
                write,
            } => Some((node, table, read_line, write_line, write)),
            _ => None,
        })
        .collect()
}

#[test]
fn read_then_write_across_an_await_is_flagged() {
    let idx = index();
    let found = findings(&idx);
    assert!(
        found.contains(&(
            "fn:api/src/lib.rs#claim".into(),
            "jobs".into(),
            4,
            9,
            "writes".into()
        )),
        "{found:?}"
    );
    assert!(
        found.contains(&(
            "fn:web/src/jobs.ts#requeue".into(),
            "jobs".into(),
            6,
            8,
            "writes".into()
        )),
        "{found:?}"
    );
}

#[test]
fn read_in_one_callee_and_delete_in_another_counts_at_the_call_lines() {
    let found = findings(&index());
    assert!(
        found.contains(&(
            "fn:api/src/lib.rs#purge".into(),
            "jobs".into(),
            47,
            49,
            "deletes".into()
        )),
        "{found:?}"
    );
}

#[test]
fn transactions_locks_other_tables_sync_code_and_tests_are_not_flagged() {
    let idx = index();
    let found = findings(&idx);
    let flagged: Vec<&str> = found.iter().map(|f| f.0.as_str()).collect();
    for quiet in [
        "fn:api/src/lib.rs#claim_in_tx",
        "fn:api/src/lib.rs#claim_locked",
        "fn:api/src/lib.rs#unrelated",
        "fn:api/src/lib.rs#write_then_read",
        "fn:api/src/lib.rs#claims_twice",
        "fn:web/src/jobs.ts#requeueInTx",
        "fn:web/src/jobs.ts#syncLookAlike",
    ] {
        assert!(!flagged.contains(&quiet), "{quiet} flagged: {flagged:?}");
    }
    assert_eq!(flagged.len(), 3, "{flagged:?}");
}

#[test]
fn begin_and_for_update_are_lock_effects_and_delete_has_its_own_kind() {
    let idx = index();
    let kinds = |id: &str| -> Vec<(EffectKind, Option<String>)> {
        idx.node(id)
            .unwrap()
            .effects
            .direct
            .iter()
            .map(|e| (e.kind, e.target.clone()))
            .collect()
    };
    assert_eq!(
        kinds("fn:api/src/lib.rs#claim_in_tx")[0],
        (EffectKind::Lock, None)
    );
    assert!(
        kinds("fn:api/src/lib.rs#claim_locked").contains(&(EffectKind::Lock, Some("jobs".into())))
    );
    assert_eq!(
        kinds("fn:web/src/jobs.ts#requeueInTx")[0],
        (EffectKind::Lock, None)
    );
    assert_eq!(
        kinds("fn:api/src/lib.rs#delete_old"),
        vec![(EffectKind::DbDelete, Some("jobs".into()))]
    );
    assert_eq!(idx.with_effect(EffectKind::DbDelete, Some("jobs")).len(), 1);
    assert!(
        idx.with_effect(EffectKind::DbWrite, Some("jobs"))
            .iter()
            .all(|n| n.id != "fn:api/src/lib.rs#delete_old")
    );
    assert!(
        idx.node("fn:api/src/lib.rs#purge")
            .unwrap()
            .effects
            .transitive
            .contains(&EffectKind::DbDelete)
    );
}
