use std::path::PathBuf;

use hoshizora_core::diff::{DanglingReason, DiffReport};
use hoshizora_core::{Index, Packs, diff};

struct Tree {
    root: PathBuf,
}

impl Tree {
    fn new(name: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("hoshizora-dangling-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("pyproject.toml"), "[project]\nname = \"app\"\n").unwrap();
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

const TACTICS: &str = "
class Tactics:
    def __init__(self, book):
        self.book = book
        self.on_hit = None

    def plan(self, t):
        return self.book

    def react(self):
        if self.on_hit:
            self.on_hit()
";

const MOUSE_BASE: &str = "
class Mouse:
    def click(self):
        pass

    def nudge_v(self, dy):
        pass
";

const MOUSE_HEAD: &str = "
class Mouse:
    def click(self):
        pass
";

const GEOMETRY: &str = "
def clamp(x, lo, hi):
    return max(lo, min(x, hi))
";

const AGENT_BASE: &str = "
import json
import threading

from app import geometry
from app.mouse import Mouse
from app.tactics import Tactics


class Agent:
    def __init__(self, book, mouse):
        self.tactics = Tactics(book)
        self.mouse = mouse
        self.callback = lambda: None

    def step(self, t):
        self.tactics.plan(t)
        self.mouse.nudge_v(4)
        self.legacy.frobnicate()
        return geometry.clamp(t, 0, 1)


class Worker(threading.Thread):
    def go(self):
        self.start()


class Base:
    def tick(self):
        self.hook()


class Child(Base):
    def hook(self):
        pass
";

const AGENT_HEAD: &str = "
import json
import threading

from app import geometry
from app.mouse import Mouse
from app.tactics import Tactics


class Agent:
    def __init__(self, book, mouse):
        self.tactics = Tactics(book)
        self.mouse = mouse
        self.callback = lambda: None

    def step(self, t):
        self.tactics.plan(t)
        self.tactics.heard(t)
        self.mouse.nudge_v(4)
        self.legacy.frobnicate()
        self.callback()
        self.settle()
        json.loads('{}')
        geometry.snap(t)
        return geometry.clamp(t, 0, 1)


class Worker(threading.Thread):
    def go(self):
        self.start()
        self.join_all()


class Base:
    def tick(self):
        self.hook()
        self.later()


class Child(Base):
    def hook(self):
        pass

    def later(self):
        pass
";

fn trees(name: &str) -> (Tree, Tree) {
    let base = Tree::new(&format!("{name}-base"));
    base.write("app/__init__.py", "")
        .write("app/tactics.py", TACTICS)
        .write("app/mouse.py", MOUSE_BASE)
        .write("app/geometry.py", GEOMETRY)
        .write("app/agent.py", AGENT_BASE);
    let head = Tree::new(&format!("{name}-head"));
    head.write("app/__init__.py", "")
        .write("app/tactics.py", TACTICS)
        .write("app/mouse.py", MOUSE_HEAD)
        .write("app/geometry.py", GEOMETRY)
        .write("app/agent.py", AGENT_HEAD);
    (base, head)
}

fn report(name: &str) -> DiffReport {
    let (base, head) = trees(name);
    diff(&base.index(), &head.index())
}

#[test]
fn reports_new_calls_to_missing_methods_and_calls_to_removed_ones() {
    let r = report("found");
    let found: Vec<(&str, DanglingReason, Option<&str>)> = r
        .dangling
        .iter()
        .map(|d| (d.callee.as_str(), d.reason, d.owner.as_deref()))
        .collect();
    assert_eq!(
        found,
        vec![
            (
                "self.tactics.heard",
                DanglingReason::Missing,
                Some("type:app/tactics.py#Tactics")
            ),
            (
                "self.mouse.nudge_v",
                DanglingReason::Removed,
                Some("type:app/mouse.py#Mouse")
            ),
            (
                "self.settle",
                DanglingReason::Missing,
                Some("type:app/agent.py#Agent")
            ),
            (
                "geometry.snap",
                DanglingReason::Missing,
                Some("file:app/geometry.py")
            ),
        ],
        "{:#?}",
        r.dangling
    );
    let removed = &r.dangling[1];
    assert_eq!(removed.function, "fn:app/agent.py#Agent.step");
    assert_eq!(
        removed.was.as_deref(),
        Some("fn:app/mouse.py#Mouse.nudge_v")
    );
}

#[test]
fn attributes_library_bases_subclasses_and_old_calls_are_not_dangling() {
    let r = report("quiet");
    for callee in [
        "self.callback",
        "self.join_all",
        "self.later",
        "json.loads",
        "self.legacy.frobnicate",
    ] {
        assert!(
            r.dangling.iter().all(|d| d.callee != callee),
            "{callee}: {:#?}",
            r.dangling
        );
    }
}

#[test]
fn unchanged_trees_have_no_dangling_calls() {
    let (base, _head) = trees("same");
    let index = base.index();
    assert!(diff(&index, &index).dangling.is_empty());
}

#[test]
fn class_attributes_are_recorded_as_fields() {
    let files = hoshizora_adapters::index_tree(&trees("fields").1.root).unwrap();
    let agent = files
        .iter()
        .find(|f| f.path == "app/agent.py")
        .unwrap()
        .symbols
        .iter()
        .find(|s| s.local_name == "Agent")
        .unwrap();
    let fields: Vec<(&str, Option<&str>)> = agent
        .params
        .iter()
        .map(|p| (p.name.as_str(), p.default.as_deref()))
        .collect();
    assert_eq!(
        fields,
        vec![
            ("tactics", Some("Tactics(book)")),
            ("mouse", Some("mouse")),
            ("callback", Some("lambda: None")),
        ]
    );
}

const SHOP_BASE: &str = "
def price(item):
    return item

def tax(x):
    return x

def total(items):
    return sum(price(i) for i in items)

def checkout(items):
    return total(items)

def receipt(items):
    return total(items)
";

const SHOP_HEAD: &str = "
def price(item):
    return item

def tax(x):
    return x

def total(items):
    return tax(sum(price(i) for i in items))

def checkout(items):
    return total(items)

def receipt(items):
    return total(items)
";

#[test]
fn callers_and_callees_of_changed_functions() {
    let base = Tree::new("shop-base");
    base.write("shop.py", SHOP_BASE);
    let head = Tree::new("shop-head");
    head.write("shop.py", SHOP_HEAD);
    let (b, h) = (base.index(), head.index());
    let r = diff(&b, &h);
    assert_eq!(r.functions.modified, vec!["fn:shop.py#total".to_owned()]);
    assert_eq!(r.callers, hoshizora_core::diff::callers(&h, &r));
    assert_eq!(
        r.callers["fn:shop.py#total"],
        vec!["fn:shop.py#checkout", "fn:shop.py#receipt"]
    );
    let callees = hoshizora_core::diff::callees(&b, &h, &r);
    assert_eq!(
        callees["fn:shop.py#total"],
        vec!["fn:shop.py#price", "fn:shop.py#tax"]
    );
}

#[test]
fn coverage_counts_the_caller_and_the_class_the_method_belongs_in() {
    use hoshizora_core::coverage::{self, Location};
    let (base, head) = trees("coverage");
    let (b, h) = (base.index(), head.index());
    let r = diff(&b, &h);
    let loc = |file: &str, line: u32| Location {
        file: file.into(),
        line,
        in_diff: false,
    };
    let case = coverage::evaluate(
        "dangling",
        &b,
        &h,
        &r,
        &[
            loc("app/tactics.py", 2),
            loc("app/agent.py", 17),
            loc("app/geometry.py", 3),
            loc("app/mouse.py", 1),
        ],
    );
    let hits: Vec<bool> = case.locations.iter().map(|l| l.dangling).collect();
    assert_eq!(hits, vec![true, true, true, false], "{:#?}", case.locations);
    assert_eq!(case.sets.dangling, r.dangling.len());
}

#[test]
fn new_conflict_markers_are_reported_and_old_ones_are_not() {
    let base = Tree::new("markers-base");
    base.write("notes.py", "X = 1\n")
        .write("doc.py", "'''\n=======\n'''\n");
    let head = Tree::new("markers-head");
    head.write(
        "notes.py",
        "<<<<<<< Updated upstream\nX = 1\n=======\nX = 2\n>>>>>>> Stashed changes\n",
    )
    .write("doc.py", "'''\n=======\n'''\n");
    let r = diff(&base.index(), &head.index());
    let found: Vec<(&str, u32)> = r
        .conflict_markers
        .iter()
        .map(|m| (m.file.as_str(), m.line))
        .collect();
    assert_eq!(
        found,
        vec![("notes.py", 1), ("notes.py", 3), ("notes.py", 5)]
    );
}
