//! tree-sitter language adapters: one source file in, [`FileFacts`] out.

use std::path::Path;

use hoshizora_model::{FileFacts, Language};
use rayon::prelude::*;

mod common;
mod go;
mod modules;
mod python;
mod rust;
mod svelte;
mod typescript;

pub use go::GoAdapter;
pub use modules::module_path;
pub use python::PythonAdapter;
pub use rust::RustAdapter;
pub use svelte::SfcAdapter;
pub use typescript::{Flavor, TsAdapter};

pub trait Adapter: Sync {
    fn language(&self) -> Language;
    /// File extensions without the dot.
    fn extensions(&self) -> &[&str];
    fn parse(&self, rel_path: &str, source: &str, module: &str) -> anyhow::Result<FileFacts>;
}

static ADAPTERS: &[&dyn Adapter] = &[
    &RustAdapter,
    &PythonAdapter,
    &TsAdapter(Flavor::TypeScript),
    &TsAdapter(Flavor::Tsx),
    &TsAdapter(Flavor::JavaScript),
    &GoAdapter,
    &SfcAdapter::SVELTE,
    &SfcAdapter::VUE,
];

pub fn adapters() -> &'static [&'static dyn Adapter] {
    ADAPTERS
}

pub fn adapter_for(path: &Path) -> Option<&'static dyn Adapter> {
    let ext = path.extension()?.to_str()?;
    ADAPTERS
        .iter()
        .copied()
        .find(|a| a.extensions().contains(&ext))
}

const SKIP_DIRS: &[&str] = &[
    "node_modules",
    "target",
    "dist",
    "build",
    "vendor",
    ".git",
    ".venv",
    "__pycache__",
];
const MAX_FILE_BYTES: u64 = 1024 * 1024;
/// Deeply nested expressions (long `+`/`||` chains) recurse once per level; default worker
/// stacks are too small for generated files.
const PARSE_STACK_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Default)]
pub struct IndexReport {
    pub facts: Vec<FileFacts>,
    /// `(relative path, error)` for files that could not be read or parsed.
    pub failures: Vec<(String, String)>,
}

/// Walks `root` (respecting .gitignore, skipping vendored/build dirs) and parses every supported file.
pub fn index_tree(root: &Path) -> anyhow::Result<Vec<FileFacts>> {
    Ok(index_tree_report(root)?.facts)
}

pub fn index_tree_report(root: &Path) -> anyhow::Result<IndexReport> {
    let root = root.canonicalize()?;
    let mut files = Vec::new();
    let walker = ignore::WalkBuilder::new(&root)
        .require_git(false)
        .filter_entry(|e| {
            !(e.file_type().is_some_and(|t| t.is_dir())
                && e.file_name()
                    .to_str()
                    .is_some_and(|n| SKIP_DIRS.contains(&n)))
        })
        .build();
    for entry in walker {
        let entry = entry?;
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.path();
        let Some(adapter) = adapter_for(path) else {
            continue;
        };
        let name = entry.file_name().to_string_lossy();
        if name.ends_with(".min.js") || name.ends_with(".min.mjs") {
            continue;
        }
        if entry
            .metadata()
            .map(|m| m.len() > MAX_FILE_BYTES)
            .unwrap_or(true)
        {
            continue;
        }
        let rel = path
            .strip_prefix(&root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        files.push((rel, path.to_path_buf(), adapter));
    }

    let pool = rayon::ThreadPoolBuilder::new()
        .stack_size(PARSE_STACK_BYTES)
        .build()?;
    let results: Vec<Result<FileFacts, (String, String)>> = pool.install(|| {
        files
            .par_iter()
            .map(|(rel, path, adapter)| {
                let source =
                    std::fs::read_to_string(path).map_err(|e| (rel.clone(), e.to_string()))?;
                let module = module_path(&root, rel, adapter.language());
                adapter
                    .parse(rel, &source, &module)
                    .map_err(|e| (rel.clone(), e.to_string()))
            })
            .collect()
    });

    let mut report = IndexReport::default();
    for r in results {
        match r {
            Ok(f) => report.facts.push(f),
            Err(e) => report.failures.push(e),
        }
    }
    report.facts.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(report)
}
