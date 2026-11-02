//! Offline evaluation of a diff report against known review corrections: which signals of the
//! report would have pointed at each correction location, and how big each signal's set is.

use std::collections::{BTreeMap, HashSet, VecDeque};

use serde::{Deserialize, Serialize};

use crate::diff::DiffReport;
use crate::index::{DEFAULT_FLOW_KINDS, Index};

pub const SIGNALS: &[&str] = &[
    "changed",
    "impact_shown",
    "impact_full",
    "effects",
    "states",
];

/// A review correction, in head coordinates. `in_diff` marks locations inside the PR's own
/// hunks; those never count towards recall.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Location {
    pub file: String,
    pub line: u32,
    #[serde(default)]
    pub in_diff: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LocationResult {
    pub file: String,
    pub line: u32,
    pub in_diff: bool,
    /// Innermost head function containing the line; every signal is false without one.
    pub function: Option<String>,
    pub changed: bool,
    /// Appears on one of the entrypoint paths the report shows (capped per changed function).
    pub impact_shown: bool,
    /// Reaches a changed function over flow edges, uncapped.
    pub impact_full: bool,
    /// Reachable from an entrypoint whose effect set changed.
    pub effects: bool,
    /// Inside a function with a state finding, or within three lines of one.
    pub states: bool,
}

/// Distinct head functions each signal covers.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SetSizes {
    pub changed: usize,
    pub impact_shown: usize,
    pub impact_full: usize,
    pub effects: usize,
    pub states: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaseResult {
    pub name: String,
    pub sets: SetSizes,
    pub locations: Vec<LocationResult>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Recall {
    pub hits: usize,
    pub total: usize,
    pub share: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SizeStats {
    pub median: usize,
    pub p90: usize,
    pub max: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Summary {
    pub cases: usize,
    pub out_of_diff_locations: usize,
    pub unmapped_locations: usize,
    pub recall: BTreeMap<String, Recall>,
    pub set_sizes: BTreeMap<String, SizeStats>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Report {
    pub summary: Summary,
    pub cases: Vec<CaseResult>,
}

struct Sets {
    changed: HashSet<String>,
    impact_shown: HashSet<String>,
    impact_full: HashSet<String>,
    effects: HashSet<String>,
    states: HashSet<String>,
    state_lines: Vec<(String, u32)>,
}

const STATE_NEAR_LINES: u32 = 3;

pub fn evaluate(
    name: &str,
    head: &Index,
    report: &DiffReport,
    locations: &[Location],
) -> CaseResult {
    let sets = signal_sets(head, report);
    let locations = locations
        .iter()
        .map(|loc| {
            let function = head.function_at(&loc.file, loc.line).map(|n| n.id.clone());
            let near_state = sets.state_lines.iter().any(|(file, line)| {
                *file == loc.file && line.abs_diff(loc.line) <= STATE_NEAR_LINES
            });
            let has = |set: &HashSet<String>| function.as_ref().is_some_and(|f| set.contains(f));
            LocationResult {
                file: loc.file.clone(),
                line: loc.line,
                in_diff: loc.in_diff,
                changed: has(&sets.changed),
                impact_shown: has(&sets.impact_shown),
                impact_full: has(&sets.impact_full),
                effects: has(&sets.effects),
                states: has(&sets.states) || near_state,
                function,
            }
        })
        .collect();
    CaseResult {
        name: name.to_owned(),
        sets: SetSizes {
            changed: sets.changed.len(),
            impact_shown: sets.impact_shown.len(),
            impact_full: sets.impact_full.len(),
            effects: sets.effects.len(),
            states: sets.states.len(),
        },
        locations,
    }
}

fn signal_sets(head: &Index, report: &DiffReport) -> Sets {
    let changed: HashSet<String> = report
        .functions
        .added
        .iter()
        .chain(&report.functions.modified)
        .chain(report.functions.moved.iter().map(|m| &m.to))
        .cloned()
        .collect();
    let impact_shown: HashSet<String> = report
        .impact
        .values()
        .flat_map(|s| &s.entrypoints)
        .flat_map(|e| e.path.iter().chain(std::iter::once(&e.node)))
        .filter(|id| !changed.contains(*id))
        .cloned()
        .collect();
    let impact_full: HashSet<String> = reach(head, changed.iter(), Direction::Callers)
        .into_iter()
        .filter(|id| !changed.contains(id))
        .collect();
    let effect_roots = report
        .entrypoint_effect_delta
        .iter()
        .chain(&report.tests_effect_delta)
        .map(|d| &d.entrypoint);
    let effects = reach(head, effect_roots, Direction::Callees);
    let sites = report.states.silent_defaults.iter().chain(
        report
            .states
            .domains
            .iter()
            .flat_map(|d| d.unhandled.iter().map(|u| &u.site)),
    );
    let mut states = HashSet::new();
    let mut state_lines = Vec::new();
    for site in sites {
        states.insert(site.function.clone());
        if let Some(file) = head.node(&site.function).and_then(|n| n.file.clone()) {
            state_lines.push((file, site.line));
        }
    }
    Sets {
        changed,
        impact_shown,
        impact_full,
        effects,
        states,
        state_lines,
    }
}

#[derive(Clone, Copy)]
enum Direction {
    Callers,
    Callees,
}

fn reach<'a>(
    index: &Index,
    roots: impl Iterator<Item = &'a String>,
    dir: Direction,
) -> HashSet<String> {
    let mut seen = vec![false; index.graph.nodes.len()];
    let mut queue: VecDeque<usize> = roots.filter_map(|id| index.idx(id)).collect();
    for &i in &queue {
        seen[i] = true;
    }
    while let Some(i) = queue.pop_front() {
        let edges = match dir {
            Direction::Callers => &index.incoming[i],
            Direction::Callees => &index.outgoing[i],
        };
        for &ei in edges {
            let edge = &index.graph.edges[ei];
            if !DEFAULT_FLOW_KINDS.contains(&edge.kind) {
                continue;
            }
            let other = match dir {
                Direction::Callers => &edge.from,
                Direction::Callees => &edge.to,
            };
            if let Some(o) = index.idx(other)
                && !seen[o]
            {
                seen[o] = true;
                queue.push_back(o);
            }
        }
    }
    seen.iter()
        .enumerate()
        .filter(|(_, s)| **s)
        .map(|(i, _)| index.graph.nodes[i].id.clone())
        .collect()
}

pub fn summarize(cases: &[CaseResult]) -> Summary {
    let out_of_diff: Vec<&LocationResult> = cases
        .iter()
        .flat_map(|c| &c.locations)
        .filter(|l| !l.in_diff)
        .collect();
    let total = out_of_diff.len();
    let recall = SIGNALS
        .iter()
        .map(|&signal| {
            let hits = out_of_diff.iter().filter(|l| hit(l, signal)).count();
            let share = if total == 0 {
                0.0
            } else {
                hits as f64 / total as f64
            };
            (signal.to_owned(), Recall { hits, total, share })
        })
        .collect();
    let set_sizes = SIGNALS
        .iter()
        .map(|&signal| {
            let mut sizes: Vec<usize> = cases.iter().map(|c| size(&c.sets, signal)).collect();
            sizes.sort_unstable();
            (signal.to_owned(), stats(&sizes))
        })
        .collect();
    Summary {
        cases: cases.len(),
        out_of_diff_locations: total,
        unmapped_locations: out_of_diff.iter().filter(|l| l.function.is_none()).count(),
        recall,
        set_sizes,
    }
}

fn hit(l: &LocationResult, signal: &str) -> bool {
    match signal {
        "changed" => l.changed,
        "impact_shown" => l.impact_shown,
        "impact_full" => l.impact_full,
        "effects" => l.effects,
        "states" => l.states,
        _ => false,
    }
}

fn size(s: &SetSizes, signal: &str) -> usize {
    match signal {
        "changed" => s.changed,
        "impact_shown" => s.impact_shown,
        "impact_full" => s.impact_full,
        "effects" => s.effects,
        "states" => s.states,
        _ => 0,
    }
}

fn stats(sorted: &[usize]) -> SizeStats {
    if sorted.is_empty() {
        return SizeStats::default();
    }
    let at =
        |q: f64| sorted[((sorted.len() as f64 * q).ceil() as usize).clamp(1, sorted.len()) - 1];
    SizeStats {
        median: at(0.5),
        p90: at(0.9),
        max: sorted[sorted.len() - 1],
    }
}

pub fn table(report: &Report) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "{:<24} {:>5} {:>5} {:>8} {:>8} {:>8} {:>8} {:>8}\n",
        "case", "locs", "ood", "changed", "shown", "full", "effects", "states"
    ));
    for c in &report.cases {
        let ood: Vec<&LocationResult> = c.locations.iter().filter(|l| !l.in_diff).collect();
        let cell = |signal: &str| {
            let hits = ood.iter().filter(|l| hit(l, signal)).count();
            format!("{hits}/{}|{}", ood.len(), size(&c.sets, signal))
        };
        out.push_str(&format!(
            "{:<24} {:>5} {:>5} {:>8} {:>8} {:>8} {:>8} {:>8}\n",
            truncate(&c.name, 24),
            c.locations.len(),
            ood.len(),
            cell("changed"),
            cell("impact_shown"),
            cell("impact_full"),
            cell("effects"),
            cell("states"),
        ));
    }
    let s = &report.summary;
    out.push_str(&format!(
        "\n{} cases, {} out-of-diff locations ({} outside any function)\n",
        s.cases, s.out_of_diff_locations, s.unmapped_locations
    ));
    out.push_str(&format!(
        "{:<14} {:>12} {:>10} {:>10} {:>10}\n",
        "signal", "recall", "median", "p90", "max"
    ));
    for &signal in SIGNALS {
        let r = &s.recall[signal];
        let z = &s.set_sizes[signal];
        out.push_str(&format!(
            "{:<14} {:>12} {:>10} {:>10} {:>10}\n",
            signal,
            format!("{}/{} ({:.0}%)", r.hits, r.total, r.share * 100.0),
            z.median,
            z.p90,
            z.max
        ));
    }
    out
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_owned()
    } else {
        let keep: String = s.chars().take(max - 1).collect();
        format!("{keep}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentiles() {
        let s = stats(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        assert_eq!((s.median, s.p90, s.max), (5, 9, 10));
        let s = stats(&[4]);
        assert_eq!((s.median, s.p90, s.max), (4, 4, 4));
        assert_eq!(stats(&[]), SizeStats::default());
    }

    #[test]
    fn empty_summary_has_every_signal() {
        let s = summarize(&[]);
        assert_eq!(s.recall.len(), SIGNALS.len());
        assert_eq!(s.recall["changed"].share, 0.0);
    }
}
