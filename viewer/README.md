# hoshizora viewer

Vite + TypeScript (no framework) front end for the `hoshizora serve` API. `npm run build`
writes `dist/` with relative asset paths so the Rust binary can embed it.

```sh
npm install
npm run dev            # http://localhost:5173, proxies /api and /v1 to 127.0.0.1:7317
npm run dev -- --open  # same, opens the browser
npm run build          # -> dist/
npm run check          # tsc --noEmit
npm run lint           # biome
npm test               # vitest
```

## Without a backend

- `http://localhost:5173/?mock` loads the hand-written polyglot fixture (`fixtures/demo.ts`)
  and a mock live stream that emits one trace every ~2 s.
- `?synthetic=20000` generates a 20k-function graph (`fixtures/synthetic.ts`) for performance work.
- Extra query flags: `level=packages|files|functions`, `maxLinks=N`, `select=<id>`, `flow=<id>`,
  `impact=<id>`, `live=1`. `VITE_MOCK=1` forces mock mode without the flag.

## Layout

| Path | Role |
| --- | --- |
| `src/types.ts` | TypeScript mirror of `crates/hoshizora-model` |
| `src/api.ts` | `Backend` interface, HTTP client, mock switch |
| `src/mock/backend.ts` | fixture-backed backend (flow/impact computed client-side) |
| `src/graph/lod.ts` | level-of-detail aggregation: packages / files / functions, expand, link cap |
| `src/graph/query.ts` | graph index, `flowFrom`, `impactOf` |
| `src/graph/flowLayout.ts` | swimlane layout for `Subgraph.steps` |
| `src/graph/scene.ts` | 3d-force-graph wrapper: cluster force, styles, glows, batched rendering |
| `src/runtime/tracker.ts` | span→node hop tracking, trace replay schedule, waterfall |
| `src/ui/*` | toolbar, side panel, legend, bottom drawer, insights/traces lists |

Keyboard: `/` search, `Esc` clear selection / leave mode, `f` flow from selected, `i` impact.
