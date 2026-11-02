# Coverage: scoring diff reports against review corrections

`hoshizora coverage` answers, offline, how often `hoshizora diff` would have pointed a reviewer
at the places a change actually needed fixing. Input is a set of labelled PRs; for each one the
tool indexes base and head, runs the diff, maps every correction location to the head function
containing it, and checks which signals of the report cover that function.

```sh
hoshizora coverage --cases cases.json --output coverage.json [--scratch DIR] [--packs DIR]
```

The human table goes to stdout; the JSON report to `--output` (or stdout after the table).

## Input

A JSON array of cases (or `{"cases": [...]}`). Each case names either a repository with two
revisions, or two prebuilt graph files:

```json
[
  {
    "name": "pr-123",
    "repo": "/srv/repos/app",
    "base": "3f2a1c0",
    "head": "9b7e4d2",
    "locations": [
      { "file": "src/handlers.rs", "line": 42 },
      { "file": "src/db.rs", "line": 7, "in_diff": true },
      { "file": "src/jobs/nightly.rs", "line": 118, "in_diff": false }
    ]
  },
  {
    "name": "pr-124",
    "base_graph": "graphs/124-base.json",
    "head_graph": "graphs/124-head.json",
    "locations": [{ "file": "web/src/api.ts", "line": 300, "in_diff": false }]
  }
]
```

- `repo`, `base`, `head`: a local clone and any two revisions `git` resolves. Each revision is
  extracted with `git archive` into `<scratch>/<repo-slug>/<rev>/` (the clone's worktree is
  never touched) and indexed like `hoshizora index`. Extracted revisions are reused on the next
  run, so pin commits rather than branch names.
- `base_graph`, `head_graph`: graphs from `hoshizora index`; `repo` is then optional and only
  used to derive `in_diff`.
- `locations`: review corrections in head coordinates, paths relative to the repository root.
  `in_diff` marks locations inside the PR's own hunks; when omitted and the case has
  `repo` + revisions it is derived from `git diff --unified=0 base head`, otherwise it defaults
  to `false`. In-diff locations are listed but never count towards recall.

## Signals

Every location is mapped to the innermost head function whose span contains the line
(`function: null` when it is outside any function, e.g. a type or a constant; such locations
count as misses). For that function:

| signal | true when the function... | set |
| --- | --- | --- |
| `changed` | is in `functions.added`, `modified` or `moved[].to` | changed functions |
| `impact_shown` | appears on one of the `impact` paths the report prints (capped at 10 entrypoints per changed function) | distinct functions on shown paths, minus changed |
| `impact_full` | reaches a changed function over flow edges, with no cap | all transitive callers of changed functions, minus changed |
| `effects` | is reachable from an entrypoint listed in `entrypoint_effect_delta` or `tests_effect_delta` | everything those entrypoints reach |
| `states` | owns a site in `states.silent_defaults` or `states.domains[].unhandled`, or the line is within 3 lines of such a site | functions with a state finding |

`impact_shown` versus `impact_full` tells whether the cap is what loses the correction, or
whether the call graph never reached it at all.

## Output

```
case                      locs   ood  changed    shown     full  effects   states
pr-123                       3     2    0/2|2    1/2|4    1/2|11   1/2|9    0/2|0

1 cases, 2 out-of-diff locations (0 outside any function)
signal               recall     median        p90        max
changed           0/2 (0%)          2          2          2
impact_shown     1/2 (50%)          4          4          4
impact_full      1/2 (50%)         11         11         11
effects          1/2 (50%)          9          9          9
states            0/2 (0%)          0          0          0
```

Per case, each signal cell is `hits/out-of-diff locations|set size`. The summary gives recall
per signal over all out-of-diff locations and the median, p90 and max set size across cases,
so a signal that recalls everything by listing the whole repo is visible as such.

The JSON report has the same numbers: `summary.recall.<signal>.{hits,total,share}`,
`summary.set_sizes.<signal>.{median,p90,max}`, and `cases[].locations[]` with the mapped
function and every signal as a boolean, for drilling into misses.
