use std::path::PathBuf;

use hoshizora_core::{Index, Packs, diff};
use hoshizora_model::{DefaultKind, Insight};

struct Tree {
    root: PathBuf,
}

impl Tree {
    fn new(name: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("hoshizora-diff-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        Self { root }
    }

    fn write(&self, rel: &str, content: &str) -> &Self {
        let p = self.root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, content).unwrap();
        self
    }

    fn index(&self) -> Index {
        let root = self.root.canonicalize().unwrap();
        let files = hoshizora_adapters::index_tree(&root).unwrap();
        Index::new(hoshizora_core::build_graph(&root, files, &Packs::builtin()))
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

const MAIN: &str = "mod db;\nmod handlers;\nfn main() { handlers::recompute(); }\n";

const DB_BASE: &str = r#"
pub fn all_stars(pool: &sqlx::PgPool) -> Vec<i32> {
    sqlx::query("SELECT id FROM stars").fetch_all(pool);
    vec![]
}
"#;

const DB_HEAD: &str = r#"
pub fn all_stars(pool: &sqlx::PgPool) -> Vec<i32> {
    sqlx::query("SELECT id FROM stars").fetch_all(pool);
    vec![]
}

pub fn delete_observations(pool: &sqlx::PgPool, star: i32) {
    sqlx::query("DELETE FROM observations WHERE star_id = $1").bind(star).execute(pool);
}
"#;

const HANDLERS_BASE: &str = r#"
use crate::db;

pub fn recompute() {
    let stars = db::all_stars(&pool());
    for s in &stars {
        helper(*s);
    }
}

fn helper(_s: i32) {}

fn pool() -> sqlx::PgPool { unimplemented!() }

#[cfg(test)]
mod tests {
    #[test]
    fn recomputes() {
        super::recompute();
    }
}
"#;

const HANDLERS_HEAD: &str = r#"
use crate::db;

pub fn recompute() {
    let stars = db::all_stars(&pool());
    for s in &stars {
        helper(*s);
        db::delete_observations(&pool(), *s);
    }
}

fn helper(_s: i32) {}

fn pool() -> sqlx::PgPool { unimplemented!() }

#[cfg(test)]
mod tests {
    #[test]
    fn recomputes() {
        super::recompute();
    }
}
"#;

fn planted_pr(name: &str) -> (Tree, Tree) {
    let base = Tree::new(&format!("{name}-base"));
    base.write("src/main.rs", MAIN)
        .write("src/db.rs", DB_BASE)
        .write("src/handlers.rs", HANDLERS_BASE);
    let head = Tree::new(&format!("{name}-head"));
    head.write("src/main.rs", MAIN)
        .write("src/db.rs", DB_HEAD)
        .write("src/handlers.rs", HANDLERS_HEAD);
    (base, head)
}

#[test]
fn added_modified_effects_tables_insights_and_impact() {
    let (base, head) = planted_pr("planted");
    let report = diff(&base.index(), &head.index());

    assert_eq!(
        report.functions.added,
        vec!["fn:src/db.rs#delete_observations"]
    );
    assert_eq!(report.functions.removed, Vec::<String>::new());
    assert_eq!(
        report.functions.modified,
        vec!["fn:src/handlers.rs#recompute"]
    );
    assert!(report.functions.moved.is_empty());

    let main_delta = report
        .entrypoint_effect_delta
        .iter()
        .find(|d| d.entrypoint == "fn:src/main.rs#main")
        .expect("main gains the write");
    assert_eq!(main_delta.added, vec!["db_write:observations"]);
    assert!(main_delta.removed.is_empty());
    assert!(
        report
            .entrypoint_effect_delta
            .iter()
            .all(|d| !d.entrypoint.ends_with("#recomputes")),
        "tests must not appear among entrypoints: {:?}",
        report.entrypoint_effect_delta
    );
    assert_eq!(report.tests_effect_delta.len(), 1);
    assert_eq!(
        report.tests_effect_delta[0].entrypoint,
        "fn:src/handlers.rs#recomputes"
    );
    assert_eq!(
        report.tests_effect_delta[0].added,
        vec!["db_write:observations"]
    );

    let observations = &report.tables_touched["observations"];
    assert_eq!(
        observations.db_write,
        vec!["fn:src/db.rs#delete_observations"]
    );
    assert!(observations.db_read.is_empty());
    assert!(!report.tables_touched.contains_key("stars"));

    assert_eq!(report.insights.new.len(), 1);
    assert!(matches!(
        &report.insights.new[0],
        Insight::IoInLoop { node, .. } if node == "fn:src/handlers.rs#recompute"
    ));
    assert!(report.insights.resolved.is_empty());

    let impact = &report.impact["fn:src/db.rs#delete_observations"];
    let nodes: Vec<&str> = impact.entrypoints.iter().map(|e| e.node.as_str()).collect();
    assert_eq!(nodes, vec!["fn:src/main.rs#main"]);
    assert_eq!(impact.entrypoints[0].kind, "main");
    assert_eq!(
        impact.entrypoints[0].path,
        vec![
            "fn:src/main.rs#main",
            "fn:src/handlers.rs#recompute",
            "fn:src/db.rs#delete_observations"
        ]
    );
    assert_eq!(impact.omitted, 0);
    assert!(report.impact.contains_key("fn:src/handlers.rs#recompute"));
}

#[test]
fn shifted_lines_are_not_modified_and_insights_are_not_new() {
    let base = Tree::new("shift-base");
    base.write("src/main.rs", MAIN)
        .write("src/db.rs", DB_HEAD)
        .write("src/handlers.rs", HANDLERS_HEAD);
    let head = Tree::new("shift-head");
    head.write("src/main.rs", MAIN)
        .write("src/db.rs", &format!("\n\n\n{DB_HEAD}"))
        .write(
            "src/handlers.rs",
            &format!("// a comment pushing everything down\n\n\n{HANDLERS_HEAD}"),
        );
    let report = diff(&base.index(), &head.index());
    assert!(report.functions.added.is_empty());
    assert!(report.functions.removed.is_empty());
    assert!(
        report.functions.modified.is_empty(),
        "{:?}",
        report.functions
    );
    assert!(report.entrypoint_effect_delta.is_empty());
    assert!(report.tests_effect_delta.is_empty());
    assert!(report.insights.new.is_empty(), "{:?}", report.insights.new);
    assert!(report.insights.resolved.is_empty());
    assert!(report.impact.is_empty());
}

#[test]
fn moved_functions_are_paired_not_added_and_removed() {
    let base = Tree::new("move-base");
    base.write("src/main.rs", MAIN)
        .write("src/db.rs", DB_HEAD)
        .write("src/handlers.rs", HANDLERS_HEAD);
    let head = Tree::new("move-head");
    head.write(
        "src/main.rs",
        "mod db;\nmod handlers;\nmod storage;\nfn main() { handlers::recompute(); }\n",
    )
    .write("src/db.rs", DB_BASE)
    .write(
        "src/storage.rs",
        "pub fn delete_observations(pool: &sqlx::PgPool, star: i32) {\n    sqlx::query(\"DELETE FROM observations WHERE star_id = $1\")\n        .bind(star)\n        .execute(pool);\n}\n",
    )
    .write(
        "src/handlers.rs",
        &HANDLERS_HEAD.replace("db::delete_observations", "crate::storage::delete_observations"),
    );
    let report = diff(&base.index(), &head.index());
    assert!(report.functions.added.is_empty(), "{:?}", report.functions);
    assert!(
        report.functions.removed.is_empty(),
        "{:?}",
        report.functions
    );
    assert_eq!(report.functions.moved.len(), 1);
    let moved = &report.functions.moved[0];
    assert_eq!(moved.from, "fn:src/db.rs#delete_observations");
    assert_eq!(moved.to, "fn:src/storage.rs#delete_observations");
    assert!(!moved.modified);
    assert_eq!(
        report.functions.modified,
        vec!["fn:src/handlers.rs#recompute"]
    );
    assert!(
        report.insights.new.is_empty(),
        "io_in_loop keyed on a moved callee is not new: {:?}",
        report.insights.new
    );
    assert!(
        report.entrypoint_effect_delta.is_empty(),
        "{:?}",
        report.entrypoint_effect_delta
    );
}

#[test]
fn moved_with_body_change_pairs_by_unique_name() {
    let base = Tree::new("rename-base");
    base.write("src/main.rs", MAIN)
        .write("src/db.rs", DB_HEAD)
        .write("src/handlers.rs", HANDLERS_HEAD);
    let head = Tree::new("rename-head");
    head.write(
        "src/main.rs",
        "mod db;\nmod handlers;\nmod storage;\nfn main() { handlers::recompute(); }\n",
    )
    .write("src/db.rs", DB_BASE)
    .write(
        "src/storage.rs",
        "pub fn delete_observations(pool: &sqlx::PgPool, star: i32) -> u64 {\n    sqlx::query(\"DELETE FROM observations WHERE star_id = $1\").bind(star).execute(pool);\n    1\n}\n",
    )
    .write(
        "src/handlers.rs",
        &HANDLERS_HEAD.replace("db::delete_observations", "crate::storage::delete_observations"),
    );
    let report = diff(&base.index(), &head.index());
    assert!(report.functions.added.is_empty(), "{:?}", report.functions);
    assert!(
        report.functions.removed.is_empty(),
        "{:?}",
        report.functions
    );
    assert_eq!(report.functions.moved.len(), 1);
    assert!(report.functions.moved[0].modified);
    assert!(
        report
            .impact
            .contains_key("fn:src/storage.rs#delete_observations"),
        "moved+modified functions count as changed"
    );
}

#[test]
fn graph_files_are_accepted_as_inputs() {
    let (base, head) = planted_pr("graphs");
    let dir = std::env::temp_dir().join(format!("hoshizora-diff-graphs-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let base_path = dir.join("base.json");
    let head_path = dir.join("head.json");
    hoshizora_core::save(base.index().graph(), &base_path).unwrap();
    hoshizora_core::save(head.index().graph(), &head_path).unwrap();
    let from_files = diff(
        &Index::new(hoshizora_core::load(&base_path).unwrap()),
        &Index::new(hoshizora_core::load(&head_path).unwrap()),
    );
    assert_eq!(from_files, diff(&base.index(), &head.index()));
    std::fs::remove_dir_all(dir).unwrap();
}

const STATUS_BASE: &str = "export type Status = 'active' | 'paused';\n";
const STATUS_HEAD: &str = "export type Status = 'active' | 'paused' | 'deleting';\n";
const BADGE: &str = r#"import { Status } from "./status";

export function badge(status: Status): string[] {
  switch (status) {
    case "active":
      return ["green"];
    case "paused":
      return ["grey"];
    default:
      return [];
  }
}

export function strict(status: Status): string {
  switch (status) {
    case "active":
      return "a";
    case "paused":
      return "p";
    default:
      throw new Error(status);
  }
}
"#;
const NEW_FN: &str = r#"import { Status } from "./status";

export function colour(status: Status): string {
  if (status === "active" || status === "deleting") {
    return "x";
  } else {
    return "";
  }
}
"#;

#[test]
fn added_domain_value_lists_sites_anywhere_that_do_not_handle_it() {
    let base = Tree::new("states-base");
    base.write("src/status.ts", STATUS_BASE)
        .write("src/badge.ts", BADGE);
    let head = Tree::new("states-head");
    head.write("src/status.ts", STATUS_HEAD)
        .write("src/badge.ts", BADGE)
        .write("src/colour.ts", NEW_FN);
    let report = diff(&base.index(), &head.index());

    assert_eq!(report.states.domains.len(), 1);
    let d = &report.states.domains[0];
    assert_eq!(d.domain, "Status");
    assert_eq!(d.added, vec!["deleting"]);
    assert!(d.removed.is_empty());
    let unhandled: Vec<(&str, &[String])> = d
        .unhandled
        .iter()
        .map(|u| (u.site.function.as_str(), u.values.as_slice()))
        .collect();
    assert_eq!(
        unhandled,
        vec![
            ("fn:src/badge.ts#badge", &["deleting".to_owned()][..]),
            ("fn:src/badge.ts#strict", &["deleting".to_owned()][..]),
        ]
    );
    assert_eq!(
        report.states.domains[0].unhandled[0].site.default,
        DefaultKind::ReturnsEmpty
    );

    let silent: Vec<&str> = report
        .states
        .silent_defaults
        .iter()
        .map(|s| s.function.as_str())
        .collect();
    assert_eq!(silent, vec!["fn:src/colour.ts#colour"]);
    assert_eq!(report.states.silent_defaults[0].missing, vec!["paused"]);

    let new_unhandled = report.insights.new.iter().any(|i| {
        matches!(i, Insight::UnhandledValues { node, missing, .. }
            if node == "fn:src/badge.ts#badge" && missing == &["deleting".to_owned()])
    });
    assert!(new_unhandled, "{:?}", report.insights.new);
}

#[test]
fn unchanged_domains_produce_no_states_delta() {
    let base = Tree::new("states-same-base");
    base.write("src/status.ts", STATUS_HEAD)
        .write("src/badge.ts", BADGE);
    let head = Tree::new("states-same-head");
    head.write("src/status.ts", STATUS_HEAD)
        .write("src/badge.ts", BADGE);
    let report = diff(&base.index(), &head.index());
    assert!(report.states.domains.is_empty());
    assert!(report.states.silent_defaults.is_empty());
}
