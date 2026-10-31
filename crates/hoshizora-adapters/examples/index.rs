//! `cargo run --release -p hoshizora-adapters --example index -- <dir|file>`
//! A directory prints indexing statistics; a single file prints its facts as JSON.

use std::path::Path;
use std::time::Instant;

use hoshizora_adapters::{adapter_for, index_tree_report, module_path};

fn main() -> anyhow::Result<()> {
    let arg = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("usage: index <dir|file>"))?;
    let path = Path::new(&arg);
    if path.is_file() {
        let adapter = adapter_for(path).ok_or_else(|| anyhow::anyhow!("unsupported extension"))?;
        let source = std::fs::read_to_string(path)?;
        let rel = path.file_name().unwrap_or_default().to_string_lossy();
        let module = module_path(
            path.parent().unwrap_or(Path::new(".")),
            &rel,
            adapter.language(),
        );
        let facts = adapter.parse(&rel, &source, &module)?;
        println!("{}", serde_json::to_string_pretty(&facts)?);
        return Ok(());
    }
    let start = Instant::now();
    let report = index_tree_report(path)?;
    let elapsed = start.elapsed();
    let symbols: usize = report.facts.iter().map(|f| f.symbols.len()).sum();
    let calls: usize = report
        .facts
        .iter()
        .flat_map(|f| &f.symbols)
        .filter_map(|s| s.body.as_ref())
        .map(|b| b.calls.len())
        .sum();
    let lines: u64 = report.facts.iter().map(|f| u64::from(f.lines)).sum();
    println!("files: {}", report.facts.len());
    println!("lines: {lines}");
    println!("symbols: {symbols}");
    println!("calls: {calls}");
    println!("failures: {}", report.failures.len());
    for (path, err) in &report.failures {
        println!("  {path}: {err}");
    }
    println!("wall: {elapsed:?}");
    Ok(())
}
