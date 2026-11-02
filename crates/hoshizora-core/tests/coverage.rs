use std::path::PathBuf;

use hoshizora_core::coverage::{self, Location};
use hoshizora_core::{Index, Packs, diff};

struct Tree {
    root: PathBuf,
}

impl Tree {
    fn new(name: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("hoshizora-cov-{name}-{}", std::process::id()));
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

const DB_BASE: &str = "pub fn all_stars(pool: &sqlx::PgPool) -> Vec<i32> {
    sqlx::query(\"SELECT id FROM stars\").fetch_all(pool);
    vec![]
}
";

const DB_HEAD: &str = "pub fn all_stars(pool: &sqlx::PgPool) -> Vec<i32> {
    sqlx::query(\"SELECT id FROM stars\").fetch_all(pool);
    vec![]
}

pub fn delete_observations(pool: &sqlx::PgPool, star: i32) {
    sqlx::query(\"DELETE FROM observations WHERE star_id = $1\").bind(star).execute(pool);
}
";

const HANDLERS_BASE: &str = "use crate::db;

pub fn recompute() {
    let stars = db::all_stars(&pool());
    for s in &stars {
        helper(*s);
    }
}

fn helper(_s: i32) {}

fn orphan() {}

fn pool() -> sqlx::PgPool { unimplemented!() }
";

const HANDLERS_HEAD: &str = "use crate::db;

pub fn recompute() {
    let stars = db::all_stars(&pool());
    for s in &stars {
        helper(*s);
        db::delete_observations(&pool(), *s);
    }
}

fn helper(_s: i32) {}

fn orphan() {}

fn pool() -> sqlx::PgPool { unimplemented!() }
";

#[test]
fn signals_per_location_and_aggregates() {
    let base = Tree::new("base");
    base.write("src/main.rs", MAIN)
        .write("src/db.rs", DB_BASE)
        .write("src/handlers.rs", HANDLERS_BASE);
    let head = Tree::new("head");
    head.write("src/main.rs", MAIN)
        .write("src/db.rs", DB_HEAD)
        .write("src/handlers.rs", HANDLERS_HEAD);
    let head_index = head.index();
    let report = diff(&base.index(), &head_index);
    assert_eq!(
        report.functions.modified,
        vec!["fn:src/handlers.rs#recompute"]
    );

    let loc = |file: &str, line: u32, in_diff: bool| Location {
        file: file.into(),
        line,
        in_diff,
    };
    let case = coverage::evaluate(
        "planted",
        &head_index,
        &report,
        &[
            loc("src/db.rs", 7, true),
            loc("src/handlers.rs", 5, false),
            loc("src/main.rs", 3, false),
            loc("src/handlers.rs", 13, false),
            loc("src/handlers.rs", 1, false),
        ],
    );
    let by_line = |line: u32, file: &str| {
        case.locations
            .iter()
            .find(|l| l.line == line && l.file == file)
            .unwrap()
    };

    let added = by_line(7, "src/db.rs");
    assert!(added.in_diff && added.changed);
    let modified = by_line(5, "src/handlers.rs");
    assert_eq!(
        modified.function.as_deref(),
        Some("fn:src/handlers.rs#recompute")
    );
    assert!(modified.changed && !modified.impact_full);
    let caller = by_line(3, "src/main.rs");
    assert_eq!(caller.function.as_deref(), Some("fn:src/main.rs#main"));
    assert!(!caller.changed, "{caller:?}");
    assert!(caller.impact_full, "{caller:?}");
    assert!(caller.impact_shown, "{caller:?}");
    assert!(caller.effects, "{caller:?}");
    let orphan = by_line(13, "src/handlers.rs");
    assert_eq!(
        orphan.function.as_deref(),
        Some("fn:src/handlers.rs#orphan")
    );
    assert!(!orphan.changed && !orphan.impact_full && !orphan.effects && !orphan.states);
    let outside = by_line(1, "src/handlers.rs");
    assert!(outside.function.is_none());

    assert_eq!(case.sets.changed, 2);
    assert!(case.sets.impact_full >= 1);
    assert!(case.sets.effects >= 2, "{:?}", case.sets);

    let summary = coverage::summarize(std::slice::from_ref(&case));
    assert_eq!(summary.out_of_diff_locations, 4);
    assert_eq!(summary.unmapped_locations, 1);
    assert_eq!(summary.recall["changed"].hits, 1);
    assert_eq!(summary.recall["impact_full"].hits, 1);
    assert_eq!(summary.recall["impact_full"].total, 4);
    assert_eq!(summary.set_sizes["changed"].median, 2);

    let report = coverage::Report {
        summary,
        cases: vec![case],
    };
    let text = coverage::table(&report);
    assert!(text.contains("planted"), "{text}");
    assert!(text.contains("impact_full"), "{text}");
    serde_json::to_string(&report).unwrap();
}
