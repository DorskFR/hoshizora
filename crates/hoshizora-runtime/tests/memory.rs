//! Heap growth under sustained ingest.

use std::collections::BTreeMap;

use hoshizora_model::{Node, RuntimeSpan, SpanKind};
use hoshizora_runtime::{NodeLookup, Runtime, RuntimeConfig};

fn rss_bytes() -> usize {
    let statm = std::fs::read_to_string("/proc/self/statm").unwrap_or_default();
    let pages: usize = statm
        .split_whitespace()
        .nth(1)
        .and_then(|p| p.parse().ok())
        .unwrap_or(0);
    pages * 4096
}

struct NoGraph;

impl NodeLookup for NoGraph {
    fn node(&self, _: &str) -> Option<&Node> {
        None
    }
    fn function_at(&self, _: &str, _: u32) -> Option<&Node> {
        None
    }
    fn functions_named(&self, _: &str) -> Vec<&Node> {
        Vec::new()
    }
    fn nodes(&self) -> &[Node] {
        &[]
    }
}

fn span(i: u64, nodes: u64) -> RuntimeSpan {
    let start = i * 10_000_000;
    let duration = 1_000 * (1 + (i * 7919) % 10_000_000);
    RuntimeSpan {
        trace_id: format!("trace-{i}"),
        span_id: format!("span-{i}"),
        parent_span_id: None,
        service: "svc".into(),
        name: format!("op{}", i % nodes),
        kind: SpanKind::Internal,
        start_unix_nanos: start,
        end_unix_nanos: start + duration,
        error: i.is_multiple_of(50),
        attrs: BTreeMap::new(),
        node: Some(format!("fn:op{}", i % nodes)),
        join: None,
    }
}

#[test]
fn rss_plateaus_under_sustained_ingest() {
    let nodes = 2_000u64;
    let config = RuntimeConfig {
        max_traces: 2_000,
        max_spans: 20_000,
        ..RuntimeConfig::default()
    };
    let base = rss_bytes();
    let mut rt = Runtime::new(config, NoGraph);
    let mut samples = Vec::new();
    for batch in 0..20u64 {
        let spans: Vec<RuntimeSpan> = (0..10_000u64)
            .map(|j| span(batch * 10_000 + j, nodes))
            .collect();
        let now = (batch + 1) * 10_000 * 10_000_000;
        rt.ingest(spans, now);
        rt.take_changed(now);
        samples.push(rss_bytes().saturating_sub(base));
    }
    let per_sample: Vec<String> = samples.iter().map(|b| format!("{}MB", b >> 20)).collect();
    eprintln!("rss growth after each 10k spans: {}", per_sample.join(" "));
    let warm = samples[9];
    let last = *samples.last().unwrap();
    eprintln!(
        "warm {}MB -> last {}MB ({nodes} nodes)",
        warm >> 20,
        last >> 20
    );
    assert!(
        last <= warm + warm / 10 + (8 << 20),
        "rss still growing: {}MB -> {}MB",
        warm >> 20,
        last >> 20
    );
    assert!(
        last < 96 << 20,
        "rss too large for {nodes} nodes: {}MB",
        last >> 20
    );
}
