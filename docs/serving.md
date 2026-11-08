# Serving projects and traces

How to run `hoshizora serve` for more than one local graph: several repositories, projects
declared at startup, following a remote, reclaiming disk, and routing OpenTelemetry spans. The
endpoints are listed in [api.md](api.md).

## Add repositories by URL

One server can hold several projects. Give it a data dir and add repositories by URL; each is
shallow-cloned and indexed in the background, and the list survives restarts:

```sh
hoshizora serve --data-dir .hoshizora
curl -X POST localhost:7317/api/projects -H 'content-type: application/json' \
     -d '{"url":"https://github.com/tokio-rs/mini-redis","services":["mini-redis"]}'
curl localhost:7317/api/projects                           # poll until status is "ready"
curl localhost:7317/api/projects/tokio-rs-mini-redis/search?q=set
```

Every `/api/...` route also exists under `/api/projects/{id}/...`; the unprefixed form is the
first project. Set `HOSHIZORA_GIT_TOKEN` to clone private GitHub repositories.

## Declare projects at startup

Projects can also be declared on the command line, so a server whose data dir is wiped (a
container without a volume) re-creates them on every start without blocking startup:

```sh
hoshizora serve --project https://github.com/tokio-rs/mini-redis@master \
                --project 'docs=./site'                    # [name=]<url>[@ref] or [name=]<path>
hoshizora serve --projects-file projects.json             # [{"url":..,"ref":..,"name":..,"services":[..]}, {"path":..}]
HOSHIZORA_PROJECTS=https://github.com/a/b,https://github.com/c/d hoshizora serve
```

A declared URL or path that is already registered keeps its clone and graph; its declared
`name`, `services` and `ref` are merged in on every start (a changed `ref` queues a rebuild), so
the flags are safe to keep when the data dir does persist.

## Follow a remote

Auto-refresh is opt-in per project: tick it on the project card, or `PATCH /api/projects/{id}`
with `{"auto_refresh": true}`; it is saved in `projects.json`. For opted-in projects, every
`--refresh-interval` minutes (`HOSHIZORA_REFRESH_INTERVAL`, default 15, 0 disables) a
`git ls-remote` compares the tracked ref with the indexed commit and rebuilds when it moved;
rebuilds run one project at a time.

A project without a declared `ref` also follows the deployed code: when its spans carry a
`service.version` and the remote has a tag `v<version>` or `<version>`, the project switches to
that tag. `GET /api/projects` shows the declared `ref`, the `tracked_ref` adopted this way, the
`indexed_ref`/`commit` the graph was built from and the last `seen_version`, so a stale graph is
visible.

## Reclaim disk

Clones are only needed while indexing and for the source panel, so on a small volume they can be
purged without losing anything: the project stays registered and its graph keeps being served,
and the next reindex clones again. Path projects are never touched.

```sh
curl localhost:7317/api/storage                                    # clone/graph bytes per project, disk free
curl -X POST localhost:7317/api/projects/tokio-rs-mini-redis/purge # {"id":..,"freed_bytes":..}
curl -X POST localhost:7317/api/purge                              # every idle git project
```

The viewer's home screen shows the same sizes with "Purge clone" and "Purge all clones" buttons.

## Send spans

hoshizora listens for OTLP/HTTP on its own port and on the standard port 4318, so stock SDKs
need no configuration. To point them at the main port:

```sh
OTEL_EXPORTER_OTLP_ENDPOINT=http://127.0.0.1:7317
OTEL_EXPORTER_OTLP_PROTOCOL=http/protobuf
```

Spans join to code through `code.function.name`, `code.file.path` + `code.line.number`
(Rust `#[tracing::instrument]` emits these through `tracing-opentelemetry`), `http.route`,
`db.collection.name` / `db.query.text`, `messaging.destination.name` and server addresses.

In the viewer, **Live** shows spans as they arrive and **Heat** colours functions by self time,
call rate or p95.

With several projects, spans are routed to the project owning their `service.name`, else by the
`x-hoshizora-project` header or `?project=`, else by where their `code.filepath` resolves.
