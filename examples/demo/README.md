# Observatory: hoshizora demo

A small polyglot "star catalog" app built to exercise every hoshizora feature with stock
OpenTelemetry instrumentation. Each service plants code patterns hoshizora should surface,
statically from the graph and dynamically from the trace overlay.

```
 browser (web, TS/Vite)  ──fetch + traceparent──►  api (Rust, axum/sqlx/reqwest)  ──reqwest + traceparent──►  worker (Python, FastAPI/psycopg)
        │                                                  │                                                          │
        └──────────── OTLP/HTTP spans ────────────► hoshizora :7317 ◄──────────────── OTLP/HTTP spans ────────────────┘
                                                           ▲
                                                 postgres (stars, observations, inventory, events)
```

| Dir | Stack | service.name | Port |
| --- | --- | --- | --- |
| `web/` | TypeScript, Vite, `@opentelemetry/sdk-trace-web` + fetch instrumentation | `observatory-web` | 5180 |
| `api/` | Rust, axum, sqlx, reqwest, `tracing-opentelemetry`, `opentelemetry-otlp` | `observatory-api` | 8081 |
| `worker/` | Python, FastAPI, psycopg, httpx, `opentelemetry-instrumentation-*` | `observatory-worker` | 8001 |
| `db/` | Postgres init SQL | | |

## Run

```sh
# 1. index the demo source and serve the graph + OTLP receiver (from the repo root)
cargo run -p hoshizora -- index examples/demo -o examples/demo/.hoshizora
cargo run -p hoshizora -- serve examples/demo/.hoshizora          # http://127.0.0.1:7317

# 2. start the app; services export to host.docker.internal:7317, the browser to localhost:7317
cd examples/demo
docker compose up --build -d

# 3. generate traffic on every flow
./load.sh                        # ./load.sh http://localhost:8081 0.5 for more

# 4. open the app and the viewer
xdg-open http://localhost:5180   # click the buttons: each click is a browser span
xdg-open http://127.0.0.1:7317   # hoshizora viewer with the live overlay

# 5. tear down
docker compose down -v
```

Overrides: `OTEL_EXPORTER_OTLP_ENDPOINT` (containers), `WEB_API_URL` / `WEB_OTLP_TRACES_URL`
(baked into the web build, so pass them to `docker compose build`).

Local dev without Docker: `cd api && cargo run`, `cd worker && uv run uvicorn app.main:app --port 8001`,
`cd web && npm install && npm run dev`. All default to `localhost` and a Postgres at
`postgres://observatory:observatory@localhost:5432/observatory`.

## Flows

| Button / request | Path through the stack |
| --- | --- |
| List stars, `GET /stars` | api: `all_stars` then `observation_count_for_star` per star |
| Launch observation, `POST /observations` | api: `star_by_name`, `inventory`, worker `GET /weather/{site}` (gather of `sky_conditions` + `seeing_index` → db), `warm_lens`, `insert_observation`, spawned `record_observation_event` → `events` |
| Recompute catalog, `POST /catalog/recompute` | api: `all_stars`, `find_duplicate_stars`, worker `POST /compute/magnitudes` (`apparent_magnitude` x20, `publish catalog.recomputed` → `events` + `pg_notify`), `update_magnitude` per star, `cache_moduli` under the mutex |
| Catalog summary, `GET /catalog/summary` | api: `tokio::join!(count_stars, count_observations)` |

## Planted patterns

| Pattern | Where | Expected hoshizora insight |
| --- | --- | --- |
| N+1 query | `api/src/handlers.rs` `list_stars`: `observation_count_for_star` awaited inside a `for` loop | static: db effect inside loop; runtime: many short sibling `SELECT ... observations` spans under one request |
| Sequential independent awaits | `api/src/handlers.rs` `launch_observation`: `db::inventory` then `fetch_weather` with no data dependency | static: parallelisable awaits; runtime: non-overlapping sibling spans |
| Correct parallelism | `api/src/handlers.rs` `catalog_summary`: `tokio::join!` | static: parallel effect; runtime: overlapping siblings (nothing to report) |
| Background task | `api/src/handlers.rs` `launch_observation` → `tokio::spawn(record_observation_event)` | `spawns` edge; runtime: span outliving the request span |
| Lock sync point | `api/src/handlers.rs` `cache_moduli`: `catalog_cache.lock()` | `locks` edge on `AppState.catalog_cache`; contention under `load.sh` |
| Quadratic on request data | `api/src/catalog.rs` `find_duplicate_stars`: nested loop over `stars` | static: loop depth 2, O(n²) estimate; runtime: self time grows with `star_count` |
| Dead function | `api/src/catalog.rs` `legacy_magnitude_correction` | `/api/dead`: unreachable from any entrypoint, never seen in traces |
| Blocking call in async | `api/src/catalog.rs` `warm_lens`: `std::thread::sleep` reached from an async handler | static: blocking effect propagated into `launch_observation`; runtime: self time with no children |
| HTTP client → route linking | `api/src/worker_client.rs` → `worker/app/main.py` routes; `web/src/api.ts` → api routes | `requests` edges joined to `handles`; runtime: one trace_id across web → api → worker via `traceparent` |
| Fan-out with `asyncio.gather` | `worker/app/main.py` `weather` | parallel effect; overlapping child spans |
| Hot function, many calls | `worker/app/photometry.py` `apparent_magnitude` called once per star per recompute | runtime: low per-call cost, high call rate → hot path, not inefficiency |
| Publish | `worker/app/db.py` `publish_event`: insert into `events` + `pg_notify` | `publishes` effect / `message` node `catalog.recomputed`; `db_write` on `events` |
| DB reads/writes by table | sqlx and psycopg spans carry `db.system.name`, `db.collection.name`, `db.query.text` | `table` nodes `stars`, `observations`, `inventory`, `events` with `reads` / `writes` edges |
| UI handler entrypoints | `web/src/main.ts` `onLaunch`, `onListStars`, `onRecompute`, `onSummary` wrapped by `tracedHandler` with `code.function.name` / `code.file.path` | `entrypoint` nodes joined to click spans |

## Verifying the export without hoshizora

Point the stack at any OTLP/HTTP receiver, e.g. an `otel/opentelemetry-collector-contrib` with the
`debug` exporter on `0.0.0.0:7317`, then run `./load.sh` and look for `observatory-api` and
`observatory-worker` spans sharing a `trace_id`. Browser spans need the receiver to answer CORS
preflight for `POST /v1/traces`: the web exporter sends credentials, so the response must echo the
request `Origin` (not `*`), set `Access-Control-Allow-Credentials: true` and allow `content-type`.

If a host firewall rejects docker-bridge → host traffic (ufw does by default), containers cannot
reach `host.docker.internal:7317`; run the receiver on the compose network instead and start the
stack with `OTEL_EXPORTER_OTLP_ENDPOINT=http://<receiver>:7317 docker compose up -d`.
