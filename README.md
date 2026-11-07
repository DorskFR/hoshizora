# hoshizora 星空

Map a codebase as a 3D sky of modules and functions, and watch OpenTelemetry traces light it up.

[![CI](https://github.com/DorskFR/hoshizora/actions/workflows/ci.yml/badge.svg)](https://github.com/DorskFR/hoshizora/actions/workflows/ci.yml)

![The hoshizora viewer on the polyglot demo: functions of a TypeScript frontend, a Rust API and a Python worker as stars, coloured by self time, with a ticker of incoming spans and the launch_observation handler selected, showing its effects and source](docs/assets/hero.png)

^ The [demo](examples/demo) at function level with live spans streaming in. Stars are coloured
by self time; the panel shows what `launch_observation` does directly (`spawn`) and through its
callees (network, db read and write, sleep, blocking).

hoshizora parses a repository with tree-sitter into packages, files, types and functions, works
out what every function does (network, SQL by table, filesystem, spawns, locks, blocking calls in
async code) and links a frontend `fetch` through the HTTP route, the handler, the SQL and the next
service into one path. Point an OpenTelemetry SDK at it and the same graph shows real requests,
with self time and percentiles per function. It is for reading an unfamiliar codebase or
reviewing a change by asking what an action triggers, without instrumenting anything first.

```sh
hoshizora index path/to/repo          # parse it, write .hoshizora/graph.json
hoshizora serve                       # API, OTLP receiver and the 3D viewer on :7317
hoshizora query flow '<node id>'      # everything a function or route triggers, as JSON
hoshizora query insights              # N+1 queries, sequential awaits, quadratic loops
hoshizora diff ./base ./head          # what a change does to effects, tables and entrypoints
```

## FAQ

- **Do I need to instrument my code?** No. Everything in the map, the stories and the queries
  comes from the source. Spans only add the runtime overlay.
- **Which languages?** Rust, Python, TypeScript/JavaScript, Go, Svelte and Vue.
- **Does it upload my code?** No. It runs locally; the only network calls are `git` clones and
  `ls-remote` for projects you add by URL.
- **Is it as precise as a compiler?** No. Calls are resolved by name and imports, and every edge
  carries a confidence. See [Limitations](#limitations).
- **How big a repo?** Its own repository (137 files, 1432 functions) indexes in about 2 s.
- **Can an agent use it?** Yes. Everything the viewer shows comes from an HTTP JSON API, and
  every query has a CLI equivalent that prints JSON.

## Install

Linux (x86_64, aarch64) and macOS (Apple silicon) binaries, with the viewer embedded, are on the
[releases page](https://github.com/DorskFR/hoshizora/releases):

```sh
gh release download -R DorskFR/hoshizora -p 'hoshizora-*-x86_64-unknown-linux-gnu.tar.gz'
tar xzf hoshizora-*.tar.gz && ./hoshizora --version
```

A container image is published as `ghcr.io/dorskfr/hoshizora:<tag>`. To build from source you need
Rust 1.90+ and Node 24; build the viewer first so the binary embeds it:

```sh
cd viewer && npm ci && npm run build && cd ..
cargo install --path crates/hoshizora
```

## Quick start

Index the polyglot demo in this repository and list what it finds:

```console
$ hoshizora index examples/demo
indexed /path/to/hoshizora/examples/demo in 60.59ms
  files      19
  nodes      150 (66 functions)
  edges      341
  dead       1
  ...
$ hoshizora query insights | jq -r '.insights[] | "\(.kind)\t\(.node)"'
inefficient	fn:api/src/catalog.rs#find_duplicate_stars
io_in_loop	fn:api/src/handlers.rs#list_stars
parallelisable	fn:api/src/handlers.rs#launch_observation
inefficient	fn:api/src/handlers.rs#launch_observation
io_in_loop	fn:api/src/handlers.rs#recompute_catalog
read_then_write	fn:api/src/handlers.rs#recompute_catalog
$ hoshizora serve
INFO graph loaded nodes=150 edges=341 root=/path/to/hoshizora/examples/demo
INFO hoshizora listening on http://127.0.0.1:7317
INFO OTLP/HTTP receiver also on http://127.0.0.1:4318/v1/traces
```

Open http://127.0.0.1:7317. The demo plants an N+1 query, sequential awaits, a blocking call in
async code and a dead function; its [README](examples/demo/README.md) runs the whole stack with
`docker compose up` so real traces flow in.

## Usage

### See what a click triggers

```console
$ hoshizora query flow 'fn:web/src/main.ts#onLaunch' | jq -r '.steps[] | "\(.depth)  \(.lane)\t\(.node)"' | head -12
0  web	fn:web/src/main.ts#onLaunch
1  web	fn:web/src/api.ts#launchObservation
1  web	fn:web/src/otel.ts#tracedHandler
2  web	fn:web/src/otel.ts#tracer
2  routes	route:POST /observations
2  web	fn:web/src/api.ts#request
3  api	fn:api/src/handlers.rs#launch_observation
4  api	fn:api/src/handlers.rs#record_observation_event
4  api	fn:api/src/handlers.rs#internal
4  api	fn:api/src/db.rs#insert_observation
4  api	fn:api/src/catalog.rs#warm_lens
4  api	fn:api/src/worker_client.rs#fetch_weather
```

The full output goes on through the worker's `GET /weather/{}` route into the Python `weather`
handler and the `stars`, `inventory`, `observations` and `events` tables.

In the viewer the same flow is a story, one lane per service:

![Story view of onLaunch: lanes for the web frontend, the Rust api, the Python worker and the database, with numbered steps from the click through POST /observations to the stars and inventory tables](docs/assets/story.png)

### Find what a change affects

```console
$ hoshizora query impact 'fn:api/src/db.rs#insert_observation' | jq -r '.entrypoints[] | "\(.kind)\t\(.node)"'
http_handler	fn:api/src/handlers.rs#launch_observation
main	fn:api/src/main.rs#main
ui_handler	fn:web/src/main.ts#onLaunch
main	fn:web/src/main.ts#<module>
```

`hoshizora diff ./base ./head` does this for a whole change: functions added, removed, modified
or moved, effects each entrypoint gains or loses, tables touched and new enum values left
unhandled. The other queries (`effects`, `dead`, `states`, `search`, `node`) are in
[docs/queries.md](docs/queries.md).

### Light it up with traces

hoshizora receives OTLP/HTTP on its port and on the standard port 4318, so a stock SDK exports to
it with no configuration. Spans join to code through `code.function.name`,
`code.file.path` + `code.line.number`, `http.route` and the `db.*` attributes. Turn on **Live**
in the viewer to see them arrive, and **Heat** to colour functions by self time, call rate or p95.

### Serve several repositories

```sh
hoshizora serve --project https://github.com/tokio-rs/mini-redis --project ./my-service
```

Each URL is shallow-cloned and indexed in the background, and the list persists in the data dir.
Declared projects, following a remote, purging clones and span routing are in
[docs/serving.md](docs/serving.md).

## Configuration

`hoshizora serve` options most setups touch (`hoshizora serve --help` lists all):

| Option | Env | Default | Effect |
|---|---|---|---|
| `--port` | | `7317` | API, viewer and OTLP port |
| `--otlp-port` | | `4318` | Extra standard OTLP/HTTP port, skipped when busy |
| `--host` | | `127.0.0.1` | Bind address; `0.0.0.0` in a container |
| `--data-dir` | | `.hoshizora` | Project list, clones and graphs |
| `--project` | `HOSHIZORA_PROJECTS` | | `[name=]<url>[@ref]` or a path to register at start |
| `--refresh-interval` | `HOSHIZORA_REFRESH_INTERVAL` | `15` | Minutes between remote checks for auto-refresh projects |
| | `HOSHIZORA_GIT_TOKEN` | | Token used to clone private GitHub repositories |

The HTTP API is in [docs/api.md](docs/api.md) and the design in
[docs/architecture.md](docs/architecture.md), with prior art in
[docs/research/prior-art.md](docs/research/prior-art.md).

## Extending

| To support | Add |
| --- | --- |
| A language | a tree-sitter grammar and one module implementing `Adapter` in `crates/hoshizora-adapters` |
| A library (what is a DB call, a spawn, an HTTP client) | rules in `packs/*.toml`, or a directory passed with `--packs` |
| A web framework's routes | a `[[route]]` rule in the same packs |

## Limitations

- Calls are resolved by name within the caller's language and the packages it imports. Edges
  carry a confidence (`exact`, `name`, `heuristic`, `observed`); edges with several candidate
  targets, and name matches for a method called on an unnamed expression (`x.get(0).start()`) or
  on a binding not named after the target's type, are kept for navigation but not used to
  propagate effects or complexity.
- Complexity is a heuristic estimate with its reasons attached, not a proof.
- Runtime stats come from spans: functions without spans are only visible statically.
- The 3D viewer needs WebGL.

## Development

```sh
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings
cd viewer && npm ci && npm run dev    # open http://localhost:5173/?mock for fixture data
```

To regenerate the README images, serve the demo and run the Playwright script (needs `ffmpeg`):

```sh
hoshizora serve --data-dir /tmp/hz --project examples/demo --port 5450 --otlp-port 5451
cd viewer && npm run readme:shots     # writes docs/assets/hero.png and story.png
```

## License

[MIT](LICENSE)
