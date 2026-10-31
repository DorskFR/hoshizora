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
hoshizora query dead
hoshizora query search  launch
```

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
