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

`id` is `owner-repo` of the URL, lowercased (`dorskfr-hoshizora`); only `https://`, `ssh://`
and `git@host:path` URLs are accepted. `ref` may be a branch or tag.

Private GitHub repositories are cloned with `HOSHIZORA_GIT_TOKEN` when set. The project list
persists in `<data-dir>/projects.json` and is restored on start.

Spans go to the project whose `services` contains the resource's `service.name`; otherwise to
the project named by the `x-hoshizora-project` header or `?project=` on `/v1/traces`; otherwise
to the project their `code.filepath` resolves in; otherwise to the first project. Each project
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
| GET | `/api/effects?kind=db_write&target=sessions` | `Node[]` with that direct effect |
| GET | `/api/insights` | `Insights`: static + runtime findings |
| GET | `/api/states?domain=` | `StatesReport`: value domains (enums, literal unions, `as const` lists, typed const blocks) with the branch sites linked to each — per site the function, line, subject, handled and missing values and what the catch-all does (`none`, `throws`, `returns_empty`, `other`). `domain` filters by name or id |

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
