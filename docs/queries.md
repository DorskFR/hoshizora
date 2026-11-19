# Queries

Every query prints JSON and reads `.hoshizora/graph.json` unless `--graph` says otherwise. The
same questions are answered over HTTP, see [api.md](api.md).

```sh
hoshizora query flow    'fn:web/src/api.ts#launchObservation'   # everything it triggers
hoshizora query impact  'fn:api/src/db.rs#insert_observation'   # everything that reaches it
hoshizora query effects db_write --target observations
hoshizora query insights
hoshizora query states --domain Status                       # branches on a value domain and what they miss
hoshizora query dead
hoshizora query search  launch
hoshizora query node    'fn:api/src/handlers.rs#list_stars'     # one node with its edges
hoshizora diff ./base ./head                                   # what a change does
hoshizora coverage --cases cases.json                          # score diffs against known review corrections
```

## diff

`diff` takes two source trees or graph files and prints:

- which functions were added, removed, modified (by body text, never by line numbers) or moved
  (same body elsewhere, or same name when unambiguous; `modified: true` when the body changed too);
- the effects each entrypoint gains or loses (tests in their own section);
- the readers and writers of every table the changed code touches;
- insights that appeared or went away;
- `callers`: for each changed function, every direct caller outside the change and outside
  tests, uncapped;
- for each changed function, the nearest user-facing entrypoints (routes, UI handlers, jobs, CLI,
  main) with one shortest path each, capped at 10 with an `omitted` count;
- a `states` section: domains whose values were added or removed, with every site in the repo
  that does not handle a new value;
- `dangling`: Python calls the change left pointing at nothing, either `missing` (a new
  `self.method()`, `self.field.method()` or `module.function()` call that no project class or
  module defines, judged over the whole class hierarchy including subclasses, with `owner` the
  type or file it was expected in) or `removed` (the function the same call reached in base is
  gone, `was` names it). Compiled and type-checked languages are left out: their build reports
  these already;
- `conflict_markers`: `<<<<<<<` / `=======` / `>>>>>>>` lines in head files that have more of
  them than in base.

## coverage

`coverage` replays `diff` over labelled PRs and reports, per signal, how many of the review
corrections outside the diff it would have pointed at and how large its set was; see
[coverage.md](coverage.md).

## insights

`query insights` lists `io_in_loop`, `parallelisable` and `read_then_write`. Static
`inefficient` findings (blocking calls in async code, loops nested three deep or over the same
collection, branching recursion) come only with `--inefficient`: on hand-checked samples of real
repositories about half of them were wrong and almost none were worth acting on. `serve` still
reports functions whose runtime self time is high and whose code is complex, and `diff` always
reports the ones in changed code.

## states

`states` collects declared value domains (Rust and Python enums, TypeScript `enum`s, string or
number literal unions and `as const` lists or maps, Go typed const blocks) and every `switch`,
`match` or `if`/`else if` chain that compares one subject against literals or members. A site is
linked to a domain by its qualifier (`Status::Active` → `Status`, nearest declaration first) or,
unqualified, when at least two handled values all belong to exactly one domain. Each site reports
the values it misses and what its catch-all does: `throws`, `returns_empty` (empty body, bare
`return`/`break`/`pass`, or an empty value) or `other`. A site whose `returns_empty` catch-all
swallows missing values is an `unhandled_values` insight, unless it handles fewer values than it
misses (a projection) or the only missing value is a neutral one (`all`, `none`, `ok`, `unknown`).
A catch-all that computes a value (`other`) is a deliberate mapping and is not reported. Most
silent catch-alls across a whole repository are deliberate, so `query insights` lists them only
with `--unhandled-values`; `diff` always reports the ones in changed code.
