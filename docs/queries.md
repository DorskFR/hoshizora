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
- for each changed function, the nearest user-facing entrypoints (routes, UI handlers, jobs, CLI,
  main) with one shortest path each, capped at 10 with an `omitted` count;
- a `states` section: domains whose values were added or removed, with every site in the repo
  that does not handle a new value.

## coverage

`coverage` replays `diff` over labelled PRs and reports, per signal, how many of the review
corrections outside the diff it would have pointed at and how large its set was; see
[coverage.md](coverage.md).

## states

`states` collects declared value domains (Rust and Python enums, TypeScript `enum`s, string or
number literal unions and `as const` lists or maps, Go typed const blocks) and every `switch`,
`match` or `if`/`else if` chain that compares one subject against literals or members. A site is
linked to a domain by its qualifier (`Status::Active` → `Status`, nearest declaration first) or,
unqualified, when at least two handled values all belong to exactly one domain. Each site reports
the values it misses and what its catch-all does: `throws`, `returns_empty` (empty body, bare
`return`/`break`/`pass`, or an empty value) or `other`. A site with missing values and a silent
catch-all is an `unhandled_values` insight.
