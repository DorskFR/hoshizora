use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RuntimeConfig {
    /// Traces kept in memory.
    pub max_traces: usize,
    /// Spans kept in memory across all traces.
    pub max_spans: usize,
    /// Sliding window for `rate_per_sec`.
    pub rate_window_secs: u64,
    /// HotPath: at least this many calls per second...
    pub hot_rate_per_sec: f64,
    /// ...with a p95 at most this long.
    pub hot_p95_nanos: u64,
    /// Inefficient: self p95 at least this many times the median self p95 of all nodes...
    pub inefficient_self_ratio: f64,
    /// ...on a function at least this cyclomatic.
    pub inefficient_cyclomatic: u32,
    /// Waiting: wait / total above this.
    pub waiting_ratio: f64,
    /// IoInLoop: a child node called more than this often under one parent span.
    pub io_in_loop_calls: usize,
    /// NeverObserved only after this much data.
    pub never_observed_after_secs: u64,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            max_traces: 2000,
            max_spans: 200_000,
            rate_window_secs: 60,
            hot_rate_per_sec: 50.0,
            hot_p95_nanos: 5_000_000,
            inefficient_self_ratio: 4.0,
            inefficient_cyclomatic: 10,
            waiting_ratio: 0.7,
            io_in_loop_calls: 3,
            never_observed_after_secs: 60,
        }
    }
}
