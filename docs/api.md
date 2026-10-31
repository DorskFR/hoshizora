# HTTP API

Served by `hoshizora serve` on `127.0.0.1:7317`. The same router also listens on the standard
OTLP/HTTP port `4318` (when free) so stock SDKs export to it with no configuration. All bodies are
JSON; types live in `crates/hoshizora-model`.

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
| GET | `/api/dead` | `Node[]` with `dead = true` |
| GET | `/api/effects?kind=db_write&target=sessions` | `Node[]` with that direct effect |
| GET | `/api/insights` | `Insights`: static + runtime findings |

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
