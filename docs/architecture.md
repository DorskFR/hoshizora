# hoshizora — architecture (draft)

hoshizora (星空, starry sky) turns a codebase into a queryable graph, then animates that graph
with live OpenTelemetry data.

```
 source repo ──► language adapter ──► raw facts ──► core ──► graph store ──► API ──► 3D viewer
                 (tree-sitter + .scm   (defs, calls,  (resolve,   (SQLite)     (HTTP)    CLI / MCP
                  queries, optional    imports, docs,  effects,
                  SCIP for precision)  branches...)    linkers,
                                                       metrics)
 running app ──► OTLP (traces, metrics, profiles) ──► runtime join ──► live overlay / stats
```

## Layers

What changes per project is split by *what it depends on*, not only by language.

| Layer | Depends on | Shape | Example |
| --- | --- | --- | --- |
| Language adapter | language | tree-sitter grammar + query files + small Rust resolver | `rust`, `python`, `typescript`, `go` |
| Precision upgrade (optional) | language | SCIP index ingested when present | `rust-analyzer scip`, `scip-python` |
| Effect packs | library | data file: call pattern → effect | `reqwest` → net, `sqlx::query` → db, `asyncio.gather` → parallel |
| Linkers | framework / protocol | rule: producer ↔ consumer match | `fetch("/api/x")` ↔ axum/FastAPI/Express route |
| Core | nothing | language-neutral graph algorithms | propagation, reachability, dead code |
| Runtime join | OTel semconv | span → node mapping | `code.function.name`, `code.file.path`, `code.line.number` |

Swapping the adapter gives structure, metrics, impact and dead code for free. Effect and
flow quality then depends on the library packs and linkers available for that stack.

## Graph model

Nodes: `package`, `module`, `file`, `type`, `function`, plus synthetic `route`, `table`,
`host`, `message`, `queue`, `entrypoint` (UI handler, CLI command, HTTP route, job, `main`).

Edges: `contains`, `calls`, `imports`, `spawns`, `awaits`, `reads`, `writes`, `requests`
(client → route), `handles` (route → fn), `sends` / `receives` (message), `locks`.

Function facts: signature, params with types, return type, docstring, span, async, visibility,
cyclomatic / cognitive complexity, loop nesting depth, recursion, direct + propagated effects,
`confidence` per edge (`exact` from SCIP/runtime, `name` from heuristic resolution).

## Questions the graph answers

| Question | Static | Runtime |
| --- | --- | --- |
| What does action X trigger | forward reachability from the entrypoint, across linkers | the recorded trace of X |
| What does changing X affect | reverse reachability → affected entrypoints, tests, routes | callers actually seen in traces |
| Inefficient code vs natural bottleneck | complexity, loop depth, I/O inside loops (N+1), alloc in loop | self time per call vs call rate vs time waiting on children |
| Where parallelism would help | sequential awaits with no data dependency, locks, channel recv | sibling spans that never overlap, critical-path analysis |
| Dead paths | unreachable from any entrypoint | never hit in coverage / traces over a window |

Bottleneck classification, per node:

- high self time per call, high complexity → **inefficient code**
- low per-call cost, very high call rate → **hot path**, scale with architecture or resources
- duration dominated by downstream I/O → **waiting**, look at the callee or parallelise
- sequential independent children → **parallelisable**

## Runtime data

- Traces: durations, waits, causality, cross-process flow via W3C `traceparent`.
- Metrics: request rates, pool saturation.
- CPU / memory per function: OTel profiles (pprof-compatible) mapped by symbol, or span
  attributes for thread CPU time and allocations recorded by an instrumentation layer.

Span → node join, in order of preference: `code.*` attributes, span name = qualified symbol,
`http.route` → route node, `db.query.text` → table nodes.

## Interfaces

API first: an HTTP JSON API served by the Rust binary (graph, node detail, source, queries,
live stream). CLI and MCP are thin clients over it.

## Known limits

- Dynamic dispatch (traits, interfaces, duck typing) makes name-based call edges fuzzy;
  SCIP and runtime traces correct them, and every edge carries its confidence.
- Big-O is a heuristic estimate with a stated basis, never a proof.
