# Seiza: prior-art research

Research date: 2026-10-31. Repo stats come from the GitHub API on that date, and READMEs and docs were fetched the same day.
"Unverified" means I read it in a search snippet or secondary source, not on the project's own page.

## The idea (for reference)

1. Per-language adapters build a code graph: module → file → type → function, with docs, params/types and spans.
2. Effect tags (network, DB tables, FS, spawn, async/parallel, locks/awaits, conditionals, deps, dead code, complexity/Big-O), propagated along the call graph.
3. Cross-process stitching: frontend fetch → backend route → handler → DB → message → other service.
4. A 3D view (constellation/city) plus a 2D flow panel, with click-through to code.
5. A live overlay from OTel traces, profiles and metrics showing hot paths, waits and bottlenecks.
6. An API, then a CLI and MCP server, for agents: impact analysis and "what does Y trigger".

---

## 1. Code knowledge graphs for AI agents (the 2025-2026 wave)

### codebase-memory-mcp (DeusData) — the closest overlap
- URL: https://github.com/DeusData/codebase-memory-mcp. About 47.9k stars, pushed 2026-10-31, MIT. Pure C backend.
- **Parsing:** 162 vendored tree-sitter grammars compiled into a single binary. It adds "Hybrid LSP" type resolution, a C reimplementation of tsserver/pyright/gopls/Roslyn/JDT/rust-analyzer-style resolution, for Py, TS/JS, PHP, C#, Go, C, C++, Java, Kotlin, Rust and Perl. It also reads package manifests and indexes IaC (Docker, Compose, K8s, Helm, Kustomize, Terraform, Ansible, CI) and docs.
- **Graph model:**
  - Nodes: Project, Folder, File, Module, Function, Route, Process, Image, Resource, Chart, Package, EnvVar, Variable.
  - Edges: CALLS, USAGE, IMPORTS, DEFINES, IMPLEMENTS, INHERITS, **HTTP_CALLS, ASYNC_CALLS, EMITS, LISTENS_ON, SPAWNS**, CONFIGURES (env reads), DATA_FLOWS (arg→param), DEPLOYS, CONNECTS_TO, CROSS_*.
  - Queries use Cypher-like syntax over SQLite.
- **Effects and cross-process (verified in README):**
  - "HTTP route ↔ call-site matching with confidence scoring".
  - gRPC, GraphQL and tRPC detection, and pub/sub (Socket.IO, EventEmitter, generic) in 8 languages.
  - Subprocess SPAWNS edges.
  - A `cross-repo-intelligence` index mode links services across repos.
  - Even `curl` calls in shell, Make and CI are mapped to K8s Service routes.
  - **No DB/table access model, no lock/await/parallelism tagging, and no Big-O.** Complexity is used only for "hotspots".
- **Visualization:** CBM Lens (open beta v0.0.1, loopback web UI).
  - The Architecture workspace is a 3D map where "folders become nested platforms", files and symbols are blocks, and height is source size. It is effectively a code city.
  - Galaxy is a 3D force graph with layer-by-layer expansion, path following and open-source.
  - The README is explicit: "Edge pulses show relationship direction; they **do not represent live execution**."
- **Agent interface:** 17 MCP tools, including `search_graph`, `trace_path` (inbound/outbound), `detect_changes` (diff → affected symbols with risk and test impact), `compare_graphs`, `get_architecture`, `ingest_traces`. Installer integrations for 45 agent clients.
- **Runtime:** `docs/RUNTIME_TRACE_MODEL.md` says verbatim: "This document is an architecture contract, not an implementation." The current `ingest_traces` handler "counts the supplied records and returns `status="accepted"` ... and an unimplemented note; ... the handler does not open or mutate a store." The planned design is a separate runtime-observation sidecar, opt-in `RUNTIME_CALL` overlay, and explicitly "no source-line attribution". **So the runtime overlay does not exist yet. It is a stub plus a spec.**
- **Takeaway:** this project already covers points 1, 3 (HTTP/gRPC/pubsub stitching), 4 (a 3D city and galaxy) and 6 (MCP impact analysis) for 160+ languages. Its gaps are a rich effect model (DB tables, locks, awaits, parallelism, Big-O) and live OTel and profile overlays. Its runtime-model doc is worth reading as a design reference.

### GitNexus (abhigyanpatwari)
- URL: https://github.com/abhigyanpatwari/GitNexus. About 48.1k stars, pushed 2026-10-31, TypeScript. **License: PolyForm Noncommercial 1.0.0** (commercial via akonlabs), so it cannot be forked for a product.
- **Parsing:** tree-sitter for about 18 languages, plus a type-resolution layer.
- **Storage:** LadybugDB (the KuzuDB fork) with vectors.
- **Graph:** symbols, calls, dependencies and "execution flows" (processes). Node and edge types aren't listed on the README.
- **Visualization:** a web explorer using Sigma.js (WebGL, 2D) and Graphology, plus AI chat.
- **MCP tools:** `query`, `context`, `impact` (upstream/downstream), `trace` (path between symbols), `detect_changes` (git-diff impact) and `rename`. Enterprise adds multi-repo graphs and PR blast radius.
- **Gaps:** no effects, no cross-service HTTP stitching in OSS, and no runtime data.
- **Takeaway:** it shows the MCP tool vocabulary agents have converged on: impact, trace, detect_changes, context.

### code-graph-rag (vitali87)
- URL: https://github.com/vitali87/code-graph-rag. About 5.3k stars, pushed 2026-10-31, MIT, Python.
- **Parsing:** tree-sitter plus compiler frontends where a toolchain exists (libclang, go/types, opt-in Roslyn, javac, Jedi). There's an ast-grep "structural-only" tier for other languages.
- **Storage:** SQLite by default, with Memgraph or Neo4j via Cypher.
- **Features:** dead code from entry points and data-flow edges. No visualizer, but it has graph export.
- **Runtime (verified):** `cgr trace` "records calls from a test run, or pulls production eBPF profiles" and merges them into the graph. It is one of the few that actually merges runtime call edges into a static graph. It does not use OTel spans.
- **Takeaway:** its tiered parsing (tree-sitter baseline, compiler frontend upgrade) is a good adapter design.

### CodeGraphContext
- URL: https://github.com/CodeGraphContext/CodeGraphContext. About 4.4k stars, pushed 2026-10-22, MIT, Python.
- **Parsing:** tree-sitter for about 25 languages, with **optional SCIP** (scip-clang, scip-dotnet) for precise calls.
- **Storage:** FalkorDB Lite, Kuzu, Ladybug or Neo4j.
- **Graph:** files, functions, classes, methods, params; edges are calls, inherits, imports.
- **Features:** dead code, cyclomatic complexity, call-chain impact. `--viz` produces a standalone HTML force or hierarchical graph.
- **Gaps:** purely static, no effects.
- **Takeaway:** a working example of tree-sitter-first parsing with SCIP-when-available.

### trace-mcp (nikolai-vysotskyi)
- URL: https://github.com/nikolai-vysotskyi/trace-mcp. About 199 stars, pushed 2026-10-31, MIT, TypeScript.
- **Parsing:** three passes.
  1. Tree-sitter per file, plus **framework plugins** (claimed 91) that extract routes, components, migrations, events, models and tests.
  2. Cross-file resolution.
  3. Optional LSP enrichment (tsserver, pyright, gopls, rust-analyzer) with confidence tiers.
- **Storage:** SQLite with FTS5.
- **Effect-adjacent features (closest to our point 2):**
  - Request-flow chains: URL → Route → Middleware → Controller → Service → View.
  - **ORM/DB models for 7 ORMs**: Eloquent, Prisma, TypeORM, Drizzle, Sequelize, Mongoose, SQLAlchemy. The schema is rebuilt from migrations.
  - Event → Listener → Job chains for Laravel, Django, NestJS, Celery and Socket.io.
  - Frontend↔backend edges through Inertia.
  - Multi-service discovery from Compose, OpenAPI, GraphQL and Protobuf contracts, with HTTP client calls matched to endpoints by confidence.
  - Source→sink taint analysis.
- **Visualization:** an optional Electron app with a cosmos.gl GPU graph (2D).
- **Agent interface:** 182 MCP tools, including `get_change_impact`, `get_subproject_impact`, `get_request_flow`, and `trace ci-report` (PR blast radius).
- **Runtime:** none. OTel appears only as outbound telemetry about its own tool calls.
- **Takeaway:** it shows the framework-plugin approach to effects (routes and ORM models) in practice. Its star count is small, but its scope is large.

### narsil-mcp (postrv) — Rust
- URL: https://github.com/postrv/narsil-mcp. About 185 stars, pushed 2026-10-07, Apache-2.0/MIT, Rust.
- **Parsing:** tree-sitter for 32 languages.
- **Analyses (some gated behind `--call-graph`):** call graph, **CFG (basic blocks/branches), DFG (reaching defs, dead stores)**, taint (SQLi, XSS, cmd injection, path traversal), cyclomatic and cognitive complexity, churn × complexity hotspots, dead code, SBOM and dependency audit.
- **Storage:** DashMap in memory, Tantivy search, optional Oxigraph RDF/SPARQL.
- **Visualization:** an optional Cytoscape.js web UI with call, import, symbol, hybrid and CFG views, plus complexity and taint overlays.
- **Gaps:** no runtime data, no effect model, no cross-service stitching.
- **Takeaway:** the best **Rust** reference for tree-sitter-based CFG, DFG and complexity across many languages. It is a crate we could study or depend on (the crate is on crates.io).

### Serena (oraios)
- URL: https://github.com/oraios/serena. About 30.9k stars, pushed 2026-10-28.
- **License:** GPL-3.0-or-later. The `solidlsp` subpackage is MIT.
- **Approach:** an LSP wrapper (SolidLSP, 40+ language servers) or a JetBrains plugin backend.
- **Graph:** none persistent. It answers symbol queries live: find symbol, references, implementations, rename, symbol-level edits.
- **Takeaway:** not a competitor. It shows that "LSP as a universal adapter" works for queries and edits but gives no graph or effects. SolidLSP (MIT) could serve as a precise-resolution backend.

### Potpie
- URL: https://github.com/potpie-ai/potpie. About 5.8k stars, pushed 2026-10-30, Apache-2.0.
- **Pivot:** it now calls itself a "living context graph" that mixes code, PRs, Linear/Jira and decisions, with a browser graph UI.
- **Parsing:** not documented on the README.
- **Takeaway:** low overlap. It is now more of an organisational memory than a code graph.

### Others in the agent-graph wave (lighter checks)
- **FalkorDB/code-graph**: about 350 stars, MIT, Python. A GraphRAG demo on FalkorDB with a 2D graph UI.
- **blarApp/blarify**: about 233 stars, MIT, Python. Builds a code graph using LSP and tree-sitter, for agents.
- **DeepWiki** (Devin) and **AsyncFuncAI/deepwiki-open** (about 18.3k stars, MIT): LLM-generated wiki pages and Mermaid diagrams. No real graph and no runtime data.
- **Aider repo map** (Aider-AI/aider, about 49.8k stars, Apache-2.0): tree-sitter "tags" ranked with PageRank into a token-budgeted map. Not re-verified this session.
- **Sourcegraph / Amp / Cody:** commercial products built on SCIP indexes.

Pattern across the wave:
- Tree-sitter is the baseline, upgraded with LSP or SCIP or a compiler frontend when available.
- Storage is SQLite, Kuzu/Ladybug or FalkorDB.
- The MCP surface is roughly `search / context / callers-callees / trace_path / impact / detect_changes`.
- **None has a working OTel span overlay, a rich effect model, or a lock/await/parallelism model.**

---

## 2. Runtime ↔ code linking

### AppMap (getappmap) — runtime recording of flows
- URLs: https://github.com/getappmap/appmap (spec, v1.14.0), appmap-js (CLI/analysis, pushed 2026-10-30), appmap-java, appmap-node, appmap-python, appmap-ruby, appmap-dotnet, appmap-react, and the VS Code and JetBrains plugins.
- **Activity:** active. All the main repos were pushed between Sep and Oct 2026. The JetBrains plugin is at 0.86.0 (2026-08). The product is now marketed as an "AI software architect" (Navie chat) with MCP serving recordings to coding agents.
- **License:** the spec repo shows no license on GitHub. Agents have historically been MIT (an older Red Hat blog says MIT + Commons Clause). **Check each repo's LICENSE before reusing anything.**
- **How it records:** language agents hook functions (bytecode instrumentation for Java, import hooks for Python and Node), plus web frameworks, HTTP clients and SQL drivers. There are request, test, process and remote recording modes. Supported: Java/Kotlin, Ruby, Python, Node; .NET, React, Swift and Go are in alpha.
- **Data format (verified on the spec page):**
  - `classMap`: a package → class → function tree with `location` `path:line`, `labels`, `comment`, `source`.
  - `events`: call/return events with `thread_id`, `parent_id`, `elapsed`, parameters and return values.
  - Typed events: `http_server_request` (with `normalized_path_info` like `/orders/{id}`), `http_client_request`, `sql_query` (`database_type`, `sql`), `message`, and exceptions.
  - Labels on functions (e.g. `security`).
  - Call events join to `classMap` entries by exact `path` + `lineno` match.
- **Visualization:** 2D: sequence diagrams, dependency map and trace view. Plus `appmap compare` for diff-based runtime change review.
- **Relevance:** **AppMap is the closest existing thing to "effects + flows from runtime".** It already has typed HTTP-in, HTTP-out and SQL events per code function, and the classMap is a lightweight code graph.
- **Gaps:**
  - It is in-process and per-language, and it records at dev or test time rather than from production OTel.
  - No 3D view and no static effect propagation.
  - Its cross-service view comes from joining separate recordings, not distributed trace context.
- **Reuse:** `classMap` + typed events is a good design to borrow, and we could *import* AppMap JSON as one runtime source.

### ExplorViz (Kiel University) — 3D software city with live traces
- URLs: https://github.com/ExplorViz (24 repos), https://explorviz.dev, and the demo at demo.explorviz.uni-kiel.de. This is an academic project going back to 2013 (VISSOFT).
- **Activity:** very active. otel-collector, trace-service, landscape-service and frontend were all pushed on 2026-10-28/29. Star counts are tiny (frontend about 6), and development happens on Kiel's GitLab and is mirrored to GitHub.
- **License:** Apache-2.0 (frontend `codemeta.json`; most repos).
- **Architecture (verified):**
  - A **custom OpenTelemetry Collector distribution** (Go, built with ocb) that "interprets [telemetry] for the purpose of visualization" and exports to Kafka.
  - Go `trace-service`, `metrics-service` and `log-service`.
  - A Java `landscape-service` on Neo4j.
  - A Java `code-analyzer` that does static analysis of git history. **Java only.**
  - A VS Code extension.
  - A `trace-generator`.
- **Frontend (verified from package.json):** React 19 + **@react-three/fiber, drei, uikit and @react-three/xr**, so three.js with VR/AR. Collaborative multi-user sessions.
- **Concept:** a landscape of applications, each rendered as a 3D software city (packages → districts, classes → buildings). Live trace communication is drawn as arcs between buildings, with heat-map overlays (2021 paper), semantic zoom and mini-maps (arXiv 2510.00003), and an embedded HTML view (2510.00004). It visualized the Linux kernel from ftrace → OTel (VISSOFT 2025), plus static evolution (arXiv 2610.09910).
- **How runtime maps to code:**
  - It needs **method-level spans**. Their 2024 paper (arXiv 2411.12380) compares OTel SDK auto-instrumentation, the inspectIT Ocelot Java agent and test instrumentation. Its conclusion: plain OTel auto-instrumentation is too coarse (library/framework spans only), so a Java agent creating per-method spans is needed.
  - The mapping relies on fully qualified names in span attributes. **The exact attribute keys are unverified (couldn't read the collector source; GitHub API rate-limited).**
  - Kieker v2 exports its records as OTel spans to feed ExplorViz.
- **Takeaway:** ExplorViz is **the closest prior art for points 4 and 5** (3D city plus live trace overlay from OTel). Its weak points:
  - It is static-analysis-light (Java only).
  - It has no effect model and no agent/MCP API.
  - The JVM/Kafka/Neo4j microservice stack is heavy.
  - Visualization is class/method granularity.
- **Its key lesson:** OTel auto-instrumentation alone does not give function-level traces. You either need method-level instrumentation, or you have to map coarse spans (HTTP, DB) onto static code nodes. **Static graph + coarse spans** is exactly the gap a static-first tool can fill.

### Kieker
- URL: https://github.com/kieker-monitoring/kieker. About 114 stars, pushed 2026-10-08, Apache-2.0, Java.
- **What it is:** an APM and trace-analysis framework from 2006 that reverse-engineers architecture from traces.
- **OTel interop:** v2 (2025) added an OTel exporter. A 2025 paper covers OTel → Kieker import, which needs `--asynchronousTrace`.
- **Takeaway:** academic reference for "architecture discovery from traces". Low reuse value for a Rust tool.

### Digma — continuous feedback (OTel → IDE code lenses)
- URL: https://github.com/digma-ai/digma (about 408 stars, **last pushed 2025-09-03**, no license file).
- **What it is:** a JetBrains plugin plus a local Docker backend. The GitHub repo contains only deployment and docker files; the backend and plugin are not open source (inferred from the repo contents).
- **How it maps OTel to code:** spans from vanilla OTel "with a few added attributes of our own" become IDE code-lens insights: bottlenecks, scaling behaviour, slow queries, N+1-like ORM issues.
- **Languages:** focused on Java/Kotlin, with .NET early access.
- **Status:** **appears stalled since 2025** (no 2026 news found). Not reusable.
- **Takeaway:** it validated the "OTel → code-level insight" idea and its insight taxonomy (scaling, N+1, bottleneck).

### Vendor code-origin features (closed SaaS)
- **Datadog Code Origin for Spans:** adds `_dd.code_origin.*` (file, line, function) to *service entry spans*. It needs Source Code Integration (git SHA + repo URL env vars), and IDE integration exists (IntelliJ, VS Code).
- **Grafana Pyroscope GitHub integration** (Grafana Cloud only): labels `service_repository`, `service_git_ref` and `service_root_path` let it fetch source and show **line-by-line profile cost** in the Function Details panel.
- **Sentry code mappings and stack-trace linking:** map stack frames to repo paths. Lightrun does dynamic logs and snapshots in the IDE. These two are from prior knowledge, not re-verified.
- **Takeaway:** the industry pattern for mapping runtime → code is (a) git repo + SHA resource attributes, plus (b) file/line/function on spans or frames. OTel has standardised the second part.

### OpenTelemetry semantic conventions that matter
- **`code.*` attributes (all Stable, verified):** `code.function.name` (fully qualified), `code.file.path`, `code.line.number`, `code.column.number`, `code.stacktrace`.
  - Deprecated keys: `code.namespace`, `code.function`, `code.filepath`, `code.lineno`, `code.column`.
  - None of them may be used on the Profile signal, because profiles carry their own locations.
- **OTel Profiles signal:** **public alpha since March 2026**. It has an OTLP profiles format, Collector support from v0.148.0, and the **opentelemetry-ebpf-profiler** as a collector receiver (Linux, many runtimes). Profile ↔ span correlation by trace_id is partly done, and span-level linking is still in progress.
- **Takeaway:** OTel can carry the mapping to code. Few auto-instrumentations emit `code.*` today, so we also need fallback matching:
  - `http.route` + `http.request.method` → static Route node.
  - `db.query.text` / `db.collection.name` / `db.operation.name` → DB-effect node.
  - `messaging.destination.name` → topic node.
  - `rpc.service` / `rpc.method` → gRPC handler node.
  - eBPF profile frames → function symbols.

### Trace/service-graph tooling (no code graph)
- **Jaeger** (about 23.4k stars, Apache-2.0) and **Tempo/OTel servicegraph connector**: service-level dependency maps with rate, error and latency per edge.
- **Coroot** (about 8k stars, Apache-2.0): eBPF service map plus profiling plus RCA.
- **Grafana Beyla / OBI** (about 2.2k stars): eBPF auto-instrumentation that emits HTTP/gRPC/SQL spans with no code changes. **Useful as a zero-touch span source for us.**
- **MCP servers over traces:** traceloop/opentelemetry-mcp-server, tobert/otlp-mcp, otel-mcp (PyPI). These let agents query traces, but there is no code graph behind them.
- **OpenTelemetry for VS Code** (SukantaSaha.opentelemetry, MIT): an embedded OTLP receiver in the editor with "Navigate To Code" via `code.*`. Seen in a search result only.
- **Multiplayer** (about 43 stars, MIT): full-stack session recording → coding agent fixes bugs. Low overlap.
- **code-graph-rag `cgr trace`** (above): merges test-run and eBPF call edges into a static graph. It is the only OSS project that actually merges runtime call edges into a static graph.

---

## 3. Code property graphs and symbol indexes (the "parse layer")

### Fraunhofer-AISEC CPG — has an effect-like "Concepts/Operations" overlay
- URL: https://github.com/Fraunhofer-AISEC/cpg. About 469 stars, pushed 2026-10-30, Apache-2.0 (CLA), Kotlin.
- **Frontends:**
  - Maintained: Java (JavaParser), C/C++ (Eclipse CDT), Python (jep), Go (libgoast), Rust, INI.
  - Incubating: JVM bytecode, LLVM IR.
  - Experimental: TypeScript (Node script), Ruby.
  - Its parsers are forgiving, so non-compiling code still parses.
- **Graph:** a labelled multigraph with AST, EOG (evaluation order), DFG and call edges. Exports to Neo4j. There's a `codyze-console` web app and a **`cpg-ai` module with MCP**.
- **Concepts overlay (verified directory listing):** `cpg-concepts` defines `Concept` and `Operation` nodes layered on code. Categories include `http`, `file`, `logging`, `memory`, `auth`, `crypto`, `config`, `flows`, `policy`, `arch`, `ontology`, `diskEncryption`, plus `GenericLLMConcept` / `GenericLLMOperation` and a `suggest-concepts` agent skill.
  - **This is essentially our "effect tagging" idea, already implemented as a graph overlay.** It has no `database` folder, though.
- **Gaps:** JVM-heavy, slow on big repos, no runtime and no 3D.
- **Takeaway:** the best design reference for the effect ontology: tags as separate overlay nodes linked to the code nodes, not boolean flags. Its "LLM suggests concepts" approach suits the long tail of frameworks.

### Joern
- URL: https://github.com/joernio/joern. About 3.6k stars, pushed 2026-10-30, Apache-2.0, Scala.
- **What it does:** CPG (AST + CFG + PDG) for C/C++, Java, JS, Python, Kotlin, binaries (Ghidra), and more. Its storage is flatgraph, it has a Scala query DSL and a server mode, and a data-flow engine with "semantics" (taint summaries for library calls).
- **Takeaway:** the gold standard for taint-style data flow, but heavyweight JVM and security-focused. Its library-semantics files are the same idea as an effect catalogue for stdlib/framework calls.

### CodeQL
- URL: https://github.com/github/codeql (queries/libraries, MIT; the CLI/engine is proprietary and free only for OSS and research).
- **What it does:** builds a relational DB per language from the compiler or extractor and queries it in QL. Its libraries already model **remote flow sources, SQL sinks, file system access and command execution** per framework: `RemoteFlowSource`, `SqlExecution`, `FileSystemAccess`, `SystemCommandExecution` concepts in the Python/JS/Go/Ruby libs. That is a ready-made, curated catalogue of "effect" APIs across many frameworks, though the license limits embedding it.
- **Takeaway:** mine it as a *reference list* of effect APIs per framework. Don't depend on it.

### SCIP, Kythe, stack-graphs, Glean (precise symbol indexes)
- **SCIP:** now at https://github.com/scip-code/scip (moved out of sourcegraph/). About 854 stars, Apache-2.0. A protobuf schema with Go and **Rust bindings**.
  - Indexers: scip-typescript, scip-python, scip-java (Java/Scala/Kotlin), rust-analyzer (native SCIP output), scip-clang, scip-ruby, scip-dotnet, scip-dart, scip-php. scip-go exists under Sourcegraph but isn't listed on that page.
  - It gives precise definitions, references and implementations, but **not call edges** per se. You derive calls from occurrences inside function ranges.
  - **Best precise-resolution backend to reuse.**
- **Kythe** (Google): about 2.2k stars, pushed 2026-09, Apache-2.0. A cross-reference graph; heavy, and Bazel-centric in practice.
- **stack-graphs** (GitHub): Rust, **archived 2025-09**. Name resolution via tree-sitter-graph rules. It is dead upstream, but the idea of incremental, file-local name binding is still useful. CodeViz v1 used it.
- **Glean** (Meta): about 1.4k stars, active. A fact DB with the Angle query language; deployment is heavy.

---

## 4. Visual code maps (2D/3D)

| Project | Status | Tech | What it shows | Runtime? |
|---|---|---|---|---|
| **CodeCharta** (MaibornWolff) | active, pushed 2026-10-30, 563★, BSD-3 | Kotlin `ccsh` analyzers + Angular web studio | 3D metric city (area/height/colour = LOC/complexity/churn), dependency graph with cycles, temporal-coupling edges, version delta, `cc.json` format, many importers (Sonar, Tokei, git, coverage), 19-language "unified parser" | no |
| **CBM Lens** (codebase-memory-mcp) | beta v0.0.1 | web, 3D | 3D folder platforms + Galaxy force graph over the same index as the MCP tools | no (pulses are static direction) |
| **ExplorViz** | active | React-three-fiber + XR | 3D city + landscape + live trace arcs, heatmaps, VR, collaboration | **yes (OTel)** |
| **GitNexus** | active | Sigma.js (2D WebGL) | symbol/call graph + chat | no |
| **trace-mcp app** | active | Electron + cosmos.gl (2D GPU) | big graph explorer | no |
| **narsil-mcp UI** | active | Cytoscape.js | call/import/CFG graphs with complexity/taint overlay | no |
| **Sourcetrail** | original archived 2021; community fork OpenSourceSourceTrail **archived 2026-09** | C++/Qt | the gold-standard 2D symbol graph + code side panel UX | no |
| **NumbatUI** (Quarkslab) | Sourcetrail fork, active (v0.1.1 Oct 2026), ~40★, GPL-3 | C++/Qt | generic graph viewer for any Sourcetrail-format DB (made via Python `numbat`) | no |
| **CodeViz** (YC S24) | pivoted | VS Code ext | v1 = tree-sitter + stack-graphs call graphs for ~15 languages; v2 (late 2025) = LLM-generated C4/PlantUML, "no more full codebase parsing" | no |
| **Swark** | ~1.7k★, AGPL-3, last push 2025-03 | VS Code + Copilot | LLM → Mermaid architecture diagram | no |
| **AppMap** | active | VS Code/JetBrains | sequence diagrams, dependency maps, trace view from recordings | **yes (recorded)** |
| **dependency-cruiser** | 7.4k★, MIT, active | JS/TS | module dependency rules + graphs | no |
| **madge** | 10.2k★, MIT, last push 2026-01 | JS/TS | module graph, cycles | no |
| **emerge** | 1.2k★, MIT, 2026-08 | Python, d3 | multi-language dependency graphs, metrics, Louvain clustering | no |
| **Gource** | 13.2k★, GPL-3 | C++/OpenGL | VCS history animation (not structure) | no |
| Hobby 3D code cities | various | three.js | e.g. bryan-uipath/code-city (functions as buildings, git churn heatmap), Code-to-City VS Code ext, JSCity, Manavarya09/code-city | no |
| aryx/codemap | ~104★ | GTK treemap | "Google Maps for code" (semgrep parsers) | no |

Takeaways:
- The 3D city metaphor is a well-studied, crowded space (CodeCharta, ExplorViz, CBM Lens and many hobby projects).
- The 2D "click node → see code" side panel is Sourcetrail's proven UX. Nobody has a maintained, polished open-source equivalent today.
- **Only ExplorViz animates *live* runtime data in 3D.**

---

## 5. Effect / capability analysis (point 2)

There is no general, multi-language "effect system for existing code" product. The pieces exist in narrower forms:

- **google/capslock** (Go, about 1.2k stars, BSD-3): reports the capabilities a Go package can reach by **following transitive calls down to privileged stdlib operations**. The categories (network, file, exec, unsafe, reflect, cgo...) are documented in the repo but not on the README I read. This is the exact "tag leaf APIs, propagate up the call graph" algorithm, but for Go and for supply-chain security.
- **cackle / cargo-acl** (Rust): user-defined API classes (`net`, `fs`, `process`) are checked per crate from build output, ignoring dead code. **cargo-capsec** (Rust) does a source scan for functions touching FS, network, env, process and FFI.
- **Fraunhofer CPG Concepts/Operations** (above): an effect ontology as a graph overlay, with LLM-assisted concept suggestion.
- **CodeQL concept libraries** (above): curated per-framework sources and sinks (remote input, SQL, FS, command exec).
- **Joern semantics, Semgrep taint mode** (semgrep about 17.4k stars, LGPL-2.1): sink and source catalogues.
- **Bearer** (about 2.8k stars): SAST that tracks *data types* flowing to DBs and third-party APIs. A privacy angle.
- **Meta Infer** (from prior knowledge, not re-verified): compositional per-function summaries; RacerD/starvation analyses detect **lock usage, races and blocking on the UI thread** for Java and C++. It is the only mature tool I know that reasons about locks and sync points.
- **trace-mcp:** ORM model extraction for 7 ORMs plus event/job chains, giving "which tables" via ORM models, not via SQL parsing.
- **Complexity:** cyclomatic and cognitive complexity are everywhere (narsil, CodeGraphContext, CodeCharta, CBM hotspots). **Heuristic Big-O** exists only as small single-language tools: MenokoOG/asymptote (Python, has an MCP mode), bigopy (Python), Complexer (C++), big0nia (PHP, cross-function nested-loop joins). Nothing multi-language or production-grade.
- **Endpoint extraction:** **OWASP Noir** (Crystal, MIT, about 1.5k stars, v1.0 May 2026) claims 262 frameworks for static HTTP endpoint extraction (path, method, params, source file), with `--include callee` and OpenAPI output. It's a good reference corpus for route-detection rules. Its framework list is on a separate docs page and wasn't verified.

## 6. Living architecture / C4 extraction

- Most C4 tools are model-as-code: Structurizr (the Java repo is archived; the DSL lives on), **LikeC4** (MIT, about 5k stars per a secondary source), C4-PlantUML, C4Drill. None infer the model from code.
- **Annotation-driven:** oselvar/c4 (TS decorators → Structurizr DSL); Structurizr component finders for Java/Go.
- **LLM-driven:** CodeViz v2, Swark, C4X, DeepWiki. These are generated, not derived, and drift or hallucinate.
- **Derived from code + infra:** codebase-memory-mcp `get_architecture` plus IaC edges (Compose/K8s/Helm/Terraform → `DEPLOYS`/`CONNECTS_TO`), and trace-mcp subproject discovery from Compose.
- **Derived from runtime:** Kieker architecture discovery, ExplorViz landscapes, Jaeger/Tempo service graphs.
- **Gap:** a C4-style container/component view *derived* from the code graph and *confirmed* by traces, not generated by an LLM.

---

## (a) Comparison table

Legend: ✅ yes, 🟡 partial/limited, ❌ no, ? unverified.
The idea's points: (1) multi-language graph, (2) effects, (3) cross-process stitching, (4) 3D + 2D + code view, (5) live runtime overlay, (6) agent API.

| Project | Lang. breadth | Parse | (1) Graph | (2) Effects | (3) Cross-process | (4) Viz | (5) Runtime overlay | (6) Agent API | License | Activity |
|---|---|---|---|---|---|---|---|---|---|---|
| **codebase-memory-mcp** | 162 grammars, LSP-lite for 11 | tree-sitter + C type resolver | ✅ | 🟡 spawn, env, pub/sub; no DB/locks | ✅ HTTP/gRPC/GraphQL/tRPC/pubsub, cross-repo, IaC | ✅ 3D city + 3D galaxy + source | ❌ `ingest_traces` is a stub, spec only | ✅ 17 MCP tools, impact/diff | MIT | very active, ~48k★ |
| **GitNexus** | ~18 | tree-sitter | ✅ | ❌ | 🟡 enterprise multi-repo | 🟡 2D Sigma | ❌ | ✅ impact/trace/diff | **PolyForm NC** | very active, ~48k★ |
| **trace-mcp** | 81 claimed | tree-sitter + LSP + 91 framework plugins | ✅ | 🟡 routes, ORM models, events/jobs, taint | ✅ fetch/axios → endpoint, OpenAPI/gRPC, Inertia | 🟡 2D GPU (Electron) | ❌ | ✅ 182 tools | MIT | active, ~200★ |
| **code-graph-rag** | ~14 + ast-grep tier | tree-sitter + compiler frontends | ✅ | ❌ | ❌ | ❌ | 🟡 `cgr trace` merges test/eBPF call edges | ✅ MCP | MIT | active, ~5k★ |
| **CodeGraphContext** | ~25 | tree-sitter + optional SCIP | ✅ | ❌ (complexity, dead code) | ❌ | 🟡 2D HTML | ❌ | ✅ | MIT | active, ~4k★ |
| **narsil-mcp** (Rust) | 32 | tree-sitter | ✅ + CFG/DFG | 🟡 taint only | ❌ | 🟡 2D Cytoscape | ❌ | ✅ 90 tools | Apache/MIT | active, ~185★ |
| **Serena** | 40+ | LSP | ❌ (live queries) | ❌ | ❌ | ❌ | 🟡 debugger (JetBrains) | ✅ | GPL-3 | active, ~31k★ |
| **Fraunhofer CPG** | ~8 | per-language compiler frontends | ✅ CPG | ✅ Concepts/Operations overlay (http, file, logging, ...) | ❌ | ❌ (Neo4j) | ❌ | 🟡 cpg-ai MCP | Apache-2 | active, ~470★ |
| **Joern / CodeQL** | many | own frontends / extractors | ✅ CPG / relational | 🟡 sources and sinks, taint | ❌ | ❌ | ❌ | 🟡 | Apache / MIT + proprietary CLI | active |
| **SCIP** | ~10 indexers | compiler-grade | 🟡 symbols/refs only | ❌ | ❌ | ❌ | ❌ | ❌ | Apache-2 | active |
| **AppMap** | Java, Py, Ruby, Node (+alpha) | runtime agents | 🟡 classMap | ✅ recorded HTTP in/out, SQL, messages, exceptions | 🟡 per recording | 🟡 2D sequence/deps | ✅ recorded (dev/test), not prod OTel | ✅ MCP/Navie | MIT? (check) | active |
| **ExplorViz** | runtime: any OTel; static: Java only | OTel collector + Java analyzer | 🟡 package/class/method | ❌ | ✅ landscape from traces | ✅ 3D city, VR, collaboration | ✅ **live OTel** (needs method-level spans) | ❌ | Apache-2 | active (academic), ~0★ |
| **CodeCharta** | 19 | own parser + importers | 🟡 file metrics + deps | ❌ | ❌ | ✅ 3D city | ❌ | ❌ | BSD-3 | active |
| **Digma** | Java/Kotlin, .NET | OTel | ❌ | 🟡 query/scaling insights | ❌ | IDE lenses | ✅ OTel → code lens | ❌ | closed | stalled 2025 |
| **Pyroscope / Datadog Code Origin** | many | profiles / spans | ❌ | ❌ | 🟡 service maps | flame graph + source | ✅ (SaaS) | ❌ | AGPL / closed | active |
| **capslock / cackle** | Go / Rust | compiler | call graph | ✅ capabilities, propagated | ❌ | ❌ | ❌ | ❌ | BSD-3 / MIT/Apache | active / slow |
| **Noir** | 262 frameworks claimed | rules + LLM fallback | ❌ | 🟡 endpoints | 🟡 server side only | ❌ | ❌ | ❌ | MIT | active |

**Does anything already do 80%?** No single project does. **codebase-memory-mcp comes closest, at roughly 50-60% of the feature list by surface area:**
- It has the multi-language graph, HTTP/gRPC/pub-sub stitching, a 3D city and galaxy with source view, and MCP impact analysis.
- It is missing a DB/table effect model, locks, awaits and parallelism, Big-O, and **any working runtime overlay**. Its `ingest_traces` is explicitly unimplemented, with a detailed spec in `docs/RUNTIME_TRACE_MODEL.md` that rules out source-line attribution.

**ExplorViz covers the runtime 3D half (points 4 and 5) but almost none of points 1, 2 and 6.** Combining the two is essentially the idea, plus a real effect model.

## (b) What is genuinely unsolved or unique

1. **A live OTel overlay on a static, function-level code graph, in OSS.**
   - ExplorViz does live OTel, but on a Java-only structure, and it needs method-level spans.
   - CBM has the static graph, but its runtime half is a stub.
   - code-graph-rag merges eBPF and test call edges, but has no live view.
   - Nobody maps **coarse, standard auto-instrumentation spans** (HTTP server/client, DB, messaging, RPC) onto static nodes using semconv keys. Those keys are `http.route`, `db.collection.name`/`db.query.text`, `messaging.destination.name`, `rpc.method` and `code.*`.
   - This is the main gap. It is also what makes the overlay work with zero custom instrumentation (Beyla/OBI or stock SDKs).
2. **A rich, propagated, multi-language effect model:**
   - DB reads and writes *per table*
   - locks, sync points and awaits
   - async spawns and parallelism
   - FS, network and process
   - conditional/branching density
   - Pieces exist per language: capslock (Go capabilities), CPG Concepts (ontology), CodeQL (sink catalogues), trace-mcp (ORM models), Infer (locks, Java/C++). No tool unifies them into one multi-language effect summary on every function node.
3. **Effects × runtime:** "this handler *statically* writes tables `orders` and `payments`, and *observed*: p99 40 ms, 80% of it waiting on lock X / on `SELECT ... orders`." No tool joins the static effect (why) with the runtime cost (how much).
4. **Wait and bottleneck visualization at code level:** showing awaits, locks and sync points as places where time accumulates, from span gaps and profiles. APM tools show this per span or service, not on a code map.
5. **Heuristic Big-O across languages:** only toy single-language tools exist (asymptote, bigopy, Complexer, big0nia).
6. **Agent queries over static + observed + effects with provenance and confidence.** Examples: "what does `POST /checkout` trigger (functions, tables, messages, downstream services), and which of those were actually observed in the last 24 h?" CBM's runtime spec has the right epistemics (keep observed separate from derived) but no implementation.

Already solved, so don't reinvent these:
- multi-language tree-sitter graphs
- HTTP route ↔ client call matching
- MCP impact / trace_path / detect_changes tools
- 3D code-city rendering
- cyclomatic complexity
- dead code

## (c) Recommendation: build narrow, compose aggressively

**Don't try to out-breadth codebase-memory-mcp or GitNexus.** They have about 48k stars each, 160+ grammars and dozens of client integrations. Competing on "parse everything, expose MCP" is a losing race. **Build Seiza around what's missing: effects plus a live runtime overlay**, and reuse everything else.

Concretely:

1. **Parse layer (compose):**
   - Rust `tree-sitter` + grammar crates as the baseline.
   - Ingest **SCIP** indexes (the scip Rust bindings) when present, for precise resolution: rust-analyzer, scip-typescript, scip-python, scip-java and scip-go all exist.
   - Follow code-graph-rag's tiering: tree-sitter first, a compiler-grade upgrade where possible.
   - Study narsil-mcp (Rust, Apache/MIT) for CFG, DFG and complexity on tree-sitter. It may be usable as a crate dependency.
   - Optional: an importer for codebase-memory-mcp's SQLite graph, as a fast way to get 160-language structure without writing adapters. Its schema moves fast, so treat this as a prototype bridge only.
2. **Effect layer (build, borrowing catalogues):**
   - Model effects as overlay nodes or edges, following Fraunhofer CPG's Concept/Operation design. Don't use flags.
   - Seed leaf-API catalogues per language and framework from CodeQL concept libraries, Joern semantics, capslock's categories, OWASP Noir's route rules and trace-mcp's ORM list. Re-encode them; don't copy code from license-restricted sources.
   - Propagate along the call graph the way capslock does (transitive reachability with a witness path).
   - Add an LLM-suggested-effect fallback for unknown APIs, like CPG's `suggest-concepts`.
3. **Stitching:** reproduce CBM's and trace-mcp's route ↔ call-site matching with confidence scores. Use OpenAPI, protobuf and AsyncAPI contracts where they exist.
4. **Runtime layer (build; the differentiator):**
   - Embed an OTLP receiver (Rust `opentelemetry-proto` + tonic), or ship a Collector exporter like ExplorViz's custom collector.
   - Map spans to nodes by priority:
     1. `code.function.name` / `code.file.path` + `code.line.number`
     2. `http.route` + method → Route node
     3. `rpc.*` → handler
     4. `db.*` → DB-effect node, which also confirms the static table effect
     5. `messaging.*` → topic
   - Keep observed data in a separate store from static facts, as CBM's spec argues.
   - Later: OTel Profiles (alpha, eBPF profiler) for CPU and wait attribution per function. Import **AppMap JSON** as a second, dev-time runtime source; its `classMap` + typed `http_*`/`sql_query` events map cleanly.
   - Recommend Beyla/OBI for zero-code span capture.
5. **Visualization (compose):** three.js via react-three-fiber (what ExplorViz uses).
   - Borrow the city and landscape metaphors, heat-map overlays and semantic zoom from ExplorViz papers and CodeCharta.
   - Borrow the 2D graph + code side panel UX from Sourcetrail.
   - The novel part is animating *span flow along static edges* and making waits visible. Spend the design time there.
6. **Agent API:** the converged MCP vocabulary (`search`, `context`, `callers/callees`, `trace_path`, `impact`, `detect_changes`), plus effect-native queries: `triggers(entry)` → {functions, tables, topics, services} with each item marked static-only or observed.

**MVP slice to prove the idea:** one realistic polyglot app (TS frontend + Python or Go backend + Postgres + a queue), using only stock OTel auto-instrumentation. Show `fetch → route → handler → SELECT orders → publish` lighting up live, with latency and waits. If that demo works and nobody else ships it (re-check CBM's `ingest_traces` progress first, since it's the obvious competitor to close this gap), the project has a real niche.

**Licensing notes:**
- GitNexus is PolyForm Noncommercial: do not fork it.
- Serena is GPL and Swark is AGPL: don't vendor them.
- codebase-memory-mcp, trace-mcp, code-graph-rag and CodeGraphContext are MIT; narsil is Apache/MIT; SCIP, CPG, Joern and ExplorViz are Apache-2; CodeCharta is BSD-3. All are safe to learn from or reuse.
- CodeQL's engine is proprietary.
