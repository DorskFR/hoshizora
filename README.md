# hoshizora 星空

Map a codebase as a 3D sky of modules and functions, see what every action triggers, and watch
real requests light it up through OpenTelemetry.

- **Static graph** for Rust, Python, TypeScript/JavaScript, Go, Svelte and Vue, parsed with
  tree-sitter: packages → files → types → functions, with signatures, parameter types,
  docstrings and source spans.
- **Effects** on every function, direct and propagated through callers: network, database reads
  and writes (with table names from the SQL), filesystem, processes, spawned work, joins, locks,
  publish/subscribe, sleeps and blocking calls in async code.
- **Cross-process flows**: frontend `fetch` → HTTP route → handler → SQL → call to another
  service, linked into one path.
- **Questions**: what does this action trigger, what does changing this affect, what is dead,
  where is I/O inside a loop, which awaits could run concurrently, what is blocking async code.
- **Runtime overlay**: an OTLP/HTTP receiver joins spans to graph nodes and streams them to the
  viewer, with self time, time spent waiting on children, percentiles and rates per function.
- **Agent first**: everything the viewer shows comes from an HTTP JSON API, and every query has
  a CLI equivalent that prints JSON.

## Quick start

```sh
cargo install --git https://github.com/DorskFR/hoshizora hoshizora   # or a release binary
hoshizora index path/to/repo -o .hoshizora/graph.json
hoshizora serve --graph .hoshizora/graph.json --root path/to/repo    # http://127.0.0.1:7317
```

One server can hold several projects. Give it a data dir and add repositories by URL; each is
shallow-cloned and indexed in the background, and the list survives restarts:

```sh
hoshizora serve --data-dir .hoshizora
curl -X POST localhost:7317/api/projects -H 'content-type: application/json' \
     -d '{"url":"https://github.com/tokio-rs/mini-redis","services":["mini-redis"]}'
curl localhost:7317/api/projects                           # poll until status is "ready"
curl localhost:7317/api/projects/tokio-rs-mini-redis/search?q=set
```

Projects can also be declared on the command line, so a server whose data dir is wiped (a
container without a volume) re-creates them on every start without blocking startup:

```sh
hoshizora serve --project https://github.com/tokio-rs/mini-redis@master \
                --project 'docs=./site'                    # [name=]<url>[@ref] or [name=]<path>
hoshizora serve --projects-file projects.json             # [{"url":..,"ref":..,"name":..,"services":[..]}, {"path":..}]
HOSHIZORA_PROJECTS=https://github.com/a/b,https://github.com/c/d hoshizora serve
```

A declared URL or path that is already registered is left as it is, so the flags are safe to
keep when the data dir does persist.

Clones are only needed while indexing and for the source panel, so on a small volume they can be
purged without losing anything: the project stays registered and its graph keeps being served,
and the next reindex clones again. Path projects are never touched.

```sh
curl localhost:7317/api/storage                                    # clone/graph bytes per project, disk free
curl -X POST localhost:7317/api/projects/tokio-rs-mini-redis/purge # {"id":..,"freed_bytes":..}
curl -X POST localhost:7317/api/purge                              # every idle git project
```

The viewer's home screen shows the same sizes with "Purge clone" and "Purge all clones" buttons.

Every `/api/...` route also exists under `/api/projects/{id}/...`; the unprefixed form is the
first project. Set `HOSHIZORA_GIT_TOKEN` to clone private GitHub repositories. Spans are routed
to the project owning their `service.name`, else by the `x-hoshizora-project` header or
`?project=`, else by where their `code.filepath` resolves.

Point any OpenTelemetry SDK at it. hoshizora also listens on the standard OTLP/HTTP port 4318, so
stock SDKs need no configuration:

```sh
OTEL_EXPORTER_OTLP_ENDPOINT=http://127.0.0.1:7317
OTEL_EXPORTER_OTLP_PROTOCOL=http/protobuf
```

Spans join to code through `code.function.name`, `code.file.path` + `code.line.number`
(Rust `#[tracing::instrument]` emits these through `tracing-opentelemetry`), `http.route`,
`db.collection.name` / `db.query.text`, `messaging.destination.name` and server addresses.

The polyglot demo in [`examples/demo`](examples/demo) runs a TypeScript frontend, a Rust API, a
Python worker, Postgres and hoshizora with one `docker compose up`, with planted N+1 queries,
sequential awaits, blocking calls and dead code to find.

## Queries

```sh
hoshizora query flow    'fn:web/src/api.ts#launchObservation'   # everything it triggers
hoshizora query impact  'fn:api/src/db.rs#insert_observation'   # everything that reaches it
hoshizora query effects db_write --target observations
hoshizora query insights
hoshizora query states --domain Status                       # branches on a value domain and what they miss
hoshizora query dead
hoshizora query search  launch
hoshizora diff ./base ./head                                   # what a change does
hoshizora coverage --cases cases.json                          # score diffs against known review corrections
```

`diff` takes two source trees or graph files and prints which functions were added, removed,
modified (by body text, never by line numbers) or moved (same body elsewhere, or same name when
unambiguous; `modified: true` when the body changed too), the effects each entrypoint gains or
loses (tests in their own section), the readers and writers of every table the changed code
touches, insights that appeared or went away, and for each changed function the nearest
user-facing entrypoints (routes, UI handlers, jobs, CLI, main) with one shortest path each,
capped at 10 with an `omitted` count, and a `states` section: domains whose values were added
or removed, with every site in the repo that does not handle a new value.

`coverage` replays `diff` over labelled PRs and reports, per signal, how many of the review
corrections outside the diff it would have pointed at and how large its set was; see
[`docs/coverage.md`](docs/coverage.md).

### States

`states` collects declared value domains — Rust and Python enums, TypeScript `enum`s, string or
number literal unions and `as const` lists or maps, Go typed const blocks — and every `switch`,
`match` or `if`/`else if` chain that compares one subject against literals or members. A site is
linked to a domain by its qualifier (`Status::Active` → `Status`, nearest declaration first) or,
unqualified, when at least two handled values all belong to exactly one domain. Each site reports
the values it misses and what its catch-all does: `throws`, `returns_empty` (empty body, bare
`return`/`break`/`pass`, or an empty value) or `other`. A site with missing values and a silent
catch-all is an `unhandled_values` insight.

The HTTP API is documented in [`docs/api.md`](docs/api.md); the architecture in
[`docs/architecture.md`](docs/architecture.md); prior art in
[`docs/research/prior-art.md`](docs/research/prior-art.md).

## Extending

| To support | Add |
| --- | --- |
| A language | a tree-sitter grammar and one module implementing `Adapter` in `crates/hoshizora-adapters` |
| A library (what is a DB call, a spawn, an HTTP client) | rules in `packs/*.toml`, or a directory passed with `--packs` |
| A web framework's routes | a `[[route]]` rule in the same packs |

## Limits

- Calls are resolved by name within the caller's language and the packages it imports. Edges
  carry a confidence (`exact`, `name`, `heuristic`, `observed`); edges with several candidate
  targets are kept for navigation but not used to propagate effects.
- Complexity is a heuristic estimate with its reasons attached, not a proof.
- Runtime stats come from spans: functions without spans are only visible statically.

## Development

```sh
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings
cd viewer && npm ci && npm run dev    # open http://localhost:5173/?mock for fixture data
```

Build the viewer before the binary to embed it: `cd viewer && npm run build`.
