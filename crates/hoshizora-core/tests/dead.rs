use std::path::Path;

use hoshizora_core::{Index, Packs, build_graph};

fn index() -> Index {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/dead");
    let files = hoshizora_adapters::index_tree(&root).unwrap();
    Index::new(build_graph(&root, files, &Packs::builtin()))
}

fn dead(index: &Index, id: &str) -> bool {
    index
        .node(id)
        .unwrap_or_else(|| panic!("no node {id}"))
        .dead
}

fn assert_live(index: &Index, ids: &[&str]) {
    let wrong: Vec<&str> = ids.iter().copied().filter(|id| dead(index, id)).collect();
    assert!(wrong.is_empty(), "reported dead but used: {wrong:?}");
}

fn assert_dead(index: &Index, ids: &[&str]) {
    let wrong: Vec<&str> = ids.iter().copied().filter(|id| !dead(index, id)).collect();
    assert!(wrong.is_empty(), "unused but not reported dead: {wrong:?}");
}

#[test]
fn sveltekit_route_modules_endpoints_and_hooks_are_roots() {
    let idx = index();
    assert_live(
        &idx,
        &[
            "fn:web/src/routes/+page.server.ts#load",
            "fn:web/src/routes/+page.server.ts#loadRows",
            "fn:web/src/routes/+page.server.ts#actions.save",
            "fn:web/src/routes/+page.server.ts#saveRow",
            "fn:web/src/routes/api/+server.ts#POST",
            "fn:web/src/routes/api/+server.ts#postBody",
            "fn:web/src/hooks.server.ts#reroute",
            "fn:web/src/hooks.server.ts#rerouted",
        ],
    );
    assert_dead(&idx, &["fn:web/src/routes/+page.server.ts#unusedRoute"]);
}

#[test]
fn svelte_markup_and_runes_reach_script_functions() {
    let idx = index();
    assert_live(
        &idx,
        &[
            "fn:web/src/lib/Panel.svelte#sortRows",
            "fn:web/src/lib/Panel.svelte#label",
            "fn:web/src/lib/Panel.svelte#attach",
            "fn:web/src/lib/Panel.svelte#commit",
            "fn:web/src/lib/Panel.svelte#persist",
            "fn:web/src/lib/Panel.svelte#hrefOf",
            "fn:web/src/lib/Panel.svelte#fallbackKey",
            "fn:web/src/lib/util.ts#fmt",
        ],
    );
    assert_dead(
        &idx,
        &[
            "fn:web/src/lib/Panel.svelte#orphan",
            "fn:web/src/lib/util.ts#neverUsed",
        ],
    );
}

#[test]
fn functions_passed_or_stored_as_values_are_used() {
    let idx = index();
    assert_live(
        &idx,
        &[
            "fn:web/src/lib/main.ts#tick",
            "fn:web/src/lib/main.ts#activate",
            "fn:web/src/lib/main.ts#exposed",
            "fn:web/src/lib/main.ts#mapped",
            "fn:py/app/cli.py#serve",
            "fn:py/app/cli.py#rank",
            "fn:py/app/util.py#run_command",
            "fn:py/app/process.py#retain",
            "fn:rs/src/main.rs#create",
            "fn:rs/src/main.rs#parse_expiry",
            "fn:rs/src/main.rs#build_pattern",
        ],
    );
    assert_dead(
        &idx,
        &[
            "fn:web/src/lib/main.ts#lonely",
            "fn:rs/src/main.rs#really_dead",
        ],
    );
}

#[test]
fn dynamic_imports_bind_the_module_they_load() {
    let idx = index();
    assert_live(
        &idx,
        &[
            "fn:web/src/lib/dyn.ts#runA",
            "fn:web/src/lib/dyn.ts#runB",
            "fn:web/src/lib/dyn.ts#runC",
        ],
    );
    assert_dead(&idx, &["fn:web/src/lib/dyn.ts#runUnused"]);
}

#[test]
fn constructors_field_initialisers_getters_and_default_exports() {
    let idx = index();
    assert_live(
        &idx,
        &[
            "fn:web/src/lib/scene.ts#Scene.bind",
            "fn:web/src/lib/scene.ts#makeTexture",
            "fn:web/src/lib/scene.ts#Estimator.lastAt",
            "fn:web/src/lib/scene.ts#Tool.mode",
            "fn:web/lint/rule.mjs#default.check",
            "fn:web/lint/ctx.mjs#walkMarkup",
            "fn:web/e2e/global-setup.ts#warmUp",
        ],
    );
    assert_dead(
        &idx,
        &[
            "fn:web/src/lib/scene.ts#Estimator.unreadGetter",
            "fn:web/lint/ctx.mjs#unusedLintHelper",
        ],
    );
}

#[test]
fn python_module_aliases_local_imports_decorators_and_nested_functions() {
    let idx = index();
    assert_live(
        &idx,
        &[
            "fn:py/app/cli.py#go",
            "fn:py/app/tasks.py#work",
            "fn:py/app/cli.py#Box.area",
            "fn:py/app/cli.py#leaf",
            "fn:py/app/extra.py#helper",
            "fn:py/app/retain.py#retain",
            "fn:py/app/tasks.py#work_page",
        ],
    );
    assert_dead(
        &idx,
        &[
            "fn:py/app/cli.py#cached",
            "fn:py/app/cli.py#truly_dead",
            "fn:py/app/cli.py#Mob.on_pet",
            "fn:py/app/tasks.py#unused_task",
            "fn:py/app/extra.py#extra_dead",
            "fn:py/app/retain.py#free_bytes",
        ],
    );
}

#[test]
fn rust_inline_modules_and_ffi_exports() {
    let idx = index();
    assert_live(
        &idx,
        &[
            "fn:rs/src/main.rs#mp4::find",
            "fn:rs/src/main.rs#mp4::read_box",
            "fn:rs/src/main.rs#exported",
            "fn:rs/src/main.rs#ffi_helper",
        ],
    );
    assert_dead(&idx, &["fn:rs/src/main.rs#find"]);
}

#[test]
fn published_package_entry_exports_and_reexports_are_roots() {
    let idx = index();
    assert_live(
        &idx,
        &[
            "fn:lib/src/core.ts#api",
            "fn:lib/src/core.ts#helper",
            "fn:lib/src/other.ts#named",
            "fn:lib/src/controller.svelte.ts#hmm.mounted",
            "fn:lib/src/theme.svelte.ts#Theme.setDefault",
        ],
    );
    assert_dead(&idx, &["fn:lib/src/other.ts#notReexported"]);
}

#[test]
fn stores_defaults_object_tables_getter_paths_and_files_named_by_path() {
    let idx = index();
    assert_live(
        &idx,
        &[
            "fn:web/src/lib/Store.svelte#history",
            "fn:web/src/lib/Store.svelte#clipRange",
            "fn:web/src/lib/Store.svelte#REASONS.other",
            "fn:web/src/lib/queries.ts#queries.forItem",
            "fn:web/src/lib/prefs.ts#defaultStorage",
            "fn:web/src/lib/prefs.ts#defaultSleep",
            "fn:web/src/lib/prefs.ts#Prefs.items",
            "fn:web/src/lib/prefs.ts#Prefs.count",
            "fn:web/src/lib/testing/appPaths.ts#resolve",
        ],
    );
    assert_dead(
        &idx,
        &[
            "fn:web/src/lib/queries.ts#queries.unusedQuery",
            "fn:web/src/lib/mocked.svelte.ts#mockedButUnused",
        ],
    );
}

#[test]
fn object_data_values_namespace_imports_and_python_entry_points() {
    let idx = index();
    assert_live(
        &idx,
        &[
            "fn:web/src/lib/features.ts#finiteWith",
            "fn:web/src/lib/features.ts#FEATURES.plural",
            "fn:web/src/lib/schema.ts#users",
            "fn:web/src/lib/schema.ts#posts",
            "fn:py/app/entry.py#run_cli",
        ],
    );
    assert_dead(&idx, &["fn:py/app/entry.py#not_an_entry"]);
}
