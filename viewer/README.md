# hoshizora viewer

Vite + TypeScript (no framework) front end for the `hoshizora serve` API. `npm run build`
writes `dist/` with relative asset paths so the Rust binary can embed it.

```sh
npm install
npm run dev            # http://localhost:5173, proxies /api and /v1 to 127.0.0.1:7317
HOSHIZORA_API=http://127.0.0.1:7399 npm run dev -- --port 5174   # proxy to another server
npm run dev -- --open  # same, opens the browser
npm run build          # -> dist/
npm run check          # tsc --noEmit
npm run lint           # biome
npm test               # vitest
```

## Without a backend

- `http://localhost:5173/?mock` opens a mock project list: `cctui` (`fixtures/cctui.ts`, with a
  hand-written spawn-session story) and `shop` (`fixtures/demo.ts`, with a mock live stream when
  `live=1`). Added repositories go through cloning → indexing → ready on a timer.
- `?synthetic=20000` generates a 20k-function graph (`fixtures/synthetic.ts`) for performance work.
- Extra query flags: `level=packages|files|functions`, `maxLinks=N`, `select=<id>`, `flow=<id>`
  (opens its story), `impact=<id>`, `live=1`. `VITE_MOCK=1` forces mock mode without the flag.

## URLs

`#/` project list · `#/p/{project}` map · `#/p/{project}/node/{id}` map with a selection ·
`#/p/{project}/story/{id}` story. Servers without `/api/projects` skip the list and use the same
paths without `/p/{project}` against the unprefixed API. When `/api/story` is missing or empty the
story is condensed in the browser from `/api/flow` and marked approximate.

## Layout

| Path | Role |
| --- | --- |
| `src/types.ts` | TypeScript mirror of `crates/hoshizora-model` |
| `src/api.ts` | `Backend` and `ProjectsApi` interfaces, HTTP clients, mock switch |
| `src/router.ts` | hash routes and the per-project API prefix |
| `src/mock/backend.ts` | fixture-backed backend (flow/impact computed client-side) |
| `src/graph/lod.ts` | level-of-detail aggregation: packages / files / functions, expand, link cap |
| `src/graph/query.ts` | graph index, `flowFrom`, `impactOf` |
| `src/graph/filters.ts` | test detection used by the map, search, insights and entrypoints |
| `src/story/build.ts` | condenses `/api/flow` into a `Story` when the server has no `/api/story` |
| `src/story/layout.ts` | sequence-diagram layout: lanes as columns, one row per step, arrow routing |
| `src/graph/scene.ts` | 3d-force-graph wrapper: cluster force, styles, glows, batched rendering |
| `src/runtime/tracker.ts` | span→node hop tracking, trace replay schedule, waterfall |
| `src/ui/*` | project home, story view, entrypoint picker, toolbar, resizable side panel, map options, drawer, insights/traces lists |

Keyboard: `/` search (`⇧⏎` opens the story of the top hit), `e` entrypoints, `s` story of the
selection, `i` impact, `+` `-` `0` zoom and fit in a story, `Esc` close / clear.
