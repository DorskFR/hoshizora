# HTTP API

Served by `hoshizora serve` on `127.0.0.1:7317`. The same router also listens on the standard
OTLP/HTTP port `4318` (when free) so stock SDKs export to it with no configuration. All bodies are
JSON; types live in `crates/hoshizora-model`.

## Projects

One server holds several projects. Every `/api/...` endpoint below is also served under
`/api/projects/{project}/...`; the unprefixed form targets the first registered project
(`404` when none is registered). Requests to a project that is still `cloning`/`indexing`
or has `failed` get `409 {"error", "status"}`. Errors are JSON `{"error": "..."}`.

| Method | Path | Response |
| --- | --- | --- |
| GET | `/api/projects` | `Project[]` |
| POST | `/api/projects` | body `AddProject` (`{url, ref?, name?, services?}`) → `202 Project`. Shallow-clones the git URL into the data dir and indexes it in the background; poll until `status` is `ready` or `failed`. Adding a URL that is already registered returns the existing project. |
| GET | `/api/projects/{project}` | `Project` |
| POST | `/api/projects/{project}/reindex` | `202 Project`: fetch the latest commit and rebuild; `409` while a rebuild is already running |
| DELETE | `/api/projects/{project}` | `204`, removes the clone and the graph |
| POST | `/api/projects/{project}/purge` | `Purged` (`{id, freed_bytes}`): deletes the clone of a git project to reclaim disk; the project stays `ready` and its graph keeps being served, `/api/source` answers `410` until a `reindex` clones again. `400` for a path project (local trees are never deleted), `409` while cloning or indexing |
| POST | `/api/purge` | `PurgeReport` (`{projects: Purged[], freed_bytes}`): purges every idle git project |
| GET | `/api/storage` | `Storage`: `projects: [{id, clone_bytes, graph_bytes}]` (`clone_bytes` is `null` without a clone), `clone_bytes` and `graph_bytes` totals, and `disk: {total_bytes, free_bytes}` of the data dir's filesystem (`null` off Unix). Walks the clones on each call; `GET /api/projects` never does |

`id` is `owner-repo` of the URL, lowercased (`dorskfr-hoshizora`); only `https://`, `ssh://`
and `git@host:path` URLs are accepted. `ref` may be a branch or tag.

Private GitHub repositories are cloned with `HOSHIZORA_GIT_TOKEN` when set. The project list
persists in `<data-dir>/projects.json` and is restored on start; a graph built by another
hoshizora version (`meta.indexer`) is served as is and rebuilt in the background. Projects declared with
`serve --project [name=]<url>[@ref]` (repeatable; also comma-separated in `HOSHIZORA_PROJECTS`)
or `--projects-file <json>` (`HOSHIZORA_PROJECTS_FILE`; an array of `AddProject` bodies or
`{path, name?}` entries) are added on start when their URL or path is not registered yet, and
clone and index in the background like a `POST`. Refs containing `/` need the file form.

Spans go to the project whose `services` contains the resource's `service.name`; otherwise to
the project named by the `x-hoshizora-project` header or `?project=` on `/v1/traces`; otherwise
to the project their `code.filepath` resolves in; otherwise to the only project. With several
projects, spans none of them claims are dropped. Each project
has its own runtime stats, traces, insights and `runtime/stream`.

## Graph

| Method | Path | Response |
| --- | --- | --- |
| GET | `/api/graph?detail=full\|slim` | `Graph`. `slim` (for the viewer) keeps the same shape but empties `function.doc/signature/params/attributes`, `function.complexity.basis` and edge `attrs` other than `candidates`, and drops `contains`, `imports` and `uses` edges (rebuild hierarchy from `node.parent`). Both are serialised once per process; all `/api` responses are gzip/br compressed when `Accept-Encoding` allows. |
| GET | `/api/nodes/{id}` | `NodeDetail` (id URL-encoded) |
| GET | `/api/source?file=&start=&end=` | `Source` |
| GET | `/api/search?q=&limit=` | `SearchHit[]` |
| GET | `/api/entrypoints` | `Node[]` with `entrypoint` set |

## Queries

| Method | Path | Response |
| --- | --- | --- |
| GET | `/api/flow?from={id}&depth=&kinds=calls,spawns,requests,handles,reads,writes,publishes` | `Subgraph`: everything `from` triggers |
| GET | `/api/impact?of={id}&depth=` | `Impact`: everything that reaches `of` |
| GET | `/api/story?from={id}&depth=` | `Story`: the flow condensed for humans — steps grouped into lanes (UI, services, databases, externals), pure helpers folded into their caller, tests and name-heuristic edges left out, at most 60 steps |
| GET | `/api/dead` | `Node[]` with `dead = true` |
| GET | `/api/effects?kind=db_write&target=sessions` | `Node[]` with that direct effect. Kinds: `network`, `db_read`, `db_write`, `db_delete` (`DELETE FROM`, ORM delete/remove), `fs_read`, `fs_write`, `process`, `spawn`, `join`, `lock` (also transaction begins and `SELECT ... FOR UPDATE`), `publish`, `subscribe`, `sleep`, `blocking` |
| GET | `/api/insights` | `Insights`: static + runtime findings. Static kinds: `io_in_loop`, `parallelisable`, `inefficient`, `unhandled_values`, `read_then_write` (a table read, then written or deleted from later in the same function across an await, with no lock or transaction before the write; reads and writes one callee away count at the line of the call). `inefficient.self_p95_nanos` is `null` until runtime spans cover the node; such entries sort after the ones with runtime evidence |
| GET | `/api/states?domain=` | `StatesReport`: value domains (enums, literal unions, `as const` lists, typed const blocks) with the branch sites linked to each — per site the function, line, subject, handled and missing values and what the catch-all does (`none`, `throws`, `returns_empty`, `other`) — plus `columns`: every `table.column` SQL outside tests sets to a quoted literal (`SET status = 'done'`, `INSERT ... VALUES (.., 'queued')`), with the writers per value, the statements comparing the column against literals (`readers`) and the ids of domains whose members cover every value written. `domain` filters domains by name or id and columns by `table`, `table.column` or linked domain |

## Runtime

| Method | Path | Response |
| --- | --- | --- |
| POST | `/v1/traces` | OTLP/HTTP trace export, `application/x-protobuf` or `application/json` |
| GET | `/api/runtime/stats` | `NodeStats[]` |
| GET | `/api/runtime/traces?limit=` | recent traces: `{ trace_id, root, spans: RuntimeSpan[] }[]` |
| GET | `/api/runtime/traces/{trace_id}` | one trace, spans joined to nodes |
| GET | `/api/runtime/stream` | SSE, one `LiveEvent` JSON per `data:` line |

Stock SDKs default to `http://localhost:4318`; otherwise set
`OTEL_EXPORTER_OTLP_ENDPOINT=http://127.0.0.1:7317` and `OTEL_EXPORTER_OTLP_PROTOCOL=http/protobuf`.
