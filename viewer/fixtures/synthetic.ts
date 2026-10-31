import type { Edge, EffectKind, Graph, GraphNode, Language } from "../src/types";

function mulberry32(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const LANGS: Language[] = ["rust", "typescript", "python", "go"];
const EFFECTS: EffectKind[] = [
  "network",
  "db_read",
  "db_write",
  "fs_read",
  "spawn",
  "lock",
  "sleep",
];

export interface SyntheticOptions {
  functions: number;
  packages?: number;
  filesPerPackage?: number;
  edgesPerFunction?: number;
  seed?: number;
}

/** Deterministic large graph for performance testing: `functions` function nodes plus files, packages and a few synthetic nodes. */
export function syntheticGraph(opts: SyntheticOptions): Graph {
  const rnd = mulberry32(opts.seed ?? 7);
  const packages = opts.packages ?? 24;
  const filesPerPackage = opts.filesPerPackage ?? 20;
  const edgesPer = opts.edgesPerFunction ?? 4;
  const nodes: GraphNode[] = [];
  const edges: Edge[] = [];
  const base = (
    id: string,
    kind: GraphNode["kind"],
    name: string,
    parent: string | null,
    lang: Language | null,
  ): GraphNode => ({
    id,
    kind,
    name,
    qualified_name: id.slice(id.indexOf(":") + 1),
    language: lang,
    file: null,
    span: null,
    parent,
    function: null,
    effects: { direct: [], transitive: [] },
    entrypoint: null,
    dead: false,
    lines: 0,
  });

  const fileIds: string[] = [];
  const fileLang = new Map<string, Language>();
  for (let p = 0; p < packages; p++) {
    const lang = LANGS[p % LANGS.length]!;
    const pid = `mod:pkg${p}`;
    nodes.push(base(pid, "package", `pkg${p}`, null, lang));
    for (let f = 0; f < filesPerPackage; f++) {
      const path = `pkg${p}/src/file${f}.${lang === "rust" ? "rs" : lang === "python" ? "py" : lang === "go" ? "go" : "ts"}`;
      const fid = `file:${path}`;
      const n = base(fid, "file", `file${f}`, pid, lang);
      n.file = path;
      nodes.push(n);
      edges.push(plain(pid, fid, "contains"));
      fileIds.push(fid);
      fileLang.set(fid, lang);
    }
  }
  const tables = Array.from({ length: 12 }, (_, i) => `table:t${i}`);
  const routes = Array.from({ length: 40 }, (_, i) => `route:GET /r${i}`);
  const hosts = Array.from({ length: 6 }, (_, i) => `host:h${i}.example.com`);
  for (const id of [...tables, ...routes, ...hosts]) {
    const kind = id.startsWith("table") ? "table" : id.startsWith("route") ? "route" : "host";
    nodes.push(base(id, kind, id.slice(id.indexOf(":") + 1), null, null));
  }

  const fnIds: string[] = [];
  for (let i = 0; i < opts.functions; i++) {
    const fid = fileIds[Math.floor(rnd() * fileIds.length)]!;
    const path = fid.slice(5);
    const id = `fn:${path}#f${i}`;
    const lines = 3 + Math.floor(rnd() ** 2 * 120);
    const start = 1 + Math.floor(rnd() * 2000);
    const n = base(id, "function", `f${i}`, fid, fileLang.get(fid)!);
    n.file = path;
    n.span = { start_line: start, end_line: start + lines - 1 };
    n.lines = lines;
    const r = rnd();
    if (r < 0.25) {
      const kind = EFFECTS[Math.floor(rnd() * EFFECTS.length)]!;
      n.effects.direct.push({
        kind,
        target: null,
        via: kind,
        line: start + 1,
        loop_depth: 0,
        awaited: true,
        rule: "synthetic",
      });
      n.effects.transitive.push(kind);
    }
    if (rnd() < 0.03) n.entrypoint = rnd() < 0.5 ? "http_handler" : "ui_handler";
    if (rnd() < 0.04) n.dead = true;
    n.function = {
      signature: `fn f${i}()`,
      params: [],
      returns: null,
      doc: null,
      is_async: rnd() < 0.5,
      is_method: false,
      visibility: "public",
      attributes: [],
      cyclomatic: 1 + Math.floor(rnd() * 12),
      branches: 0,
      loops: 0,
      max_loop_depth: 0,
      recursive: false,
      complexity: { time: "O(n)", space: "O(1)", basis: ["synthetic"] },
      fan_in: 0,
      fan_out: edgesPer,
    };
    nodes.push(n);
    edges.push(plain(fid, id, "contains"));
    fnIds.push(id);
  }
  for (let i = 0; i < fnIds.length; i++) {
    const from = fnIds[i]!;
    for (let k = 0; k < edgesPer; k++) {
      const local = rnd() < 0.8;
      const j = local
        ? Math.min(fnIds.length - 1, Math.max(0, i + Math.floor((rnd() - 0.5) * 60)))
        : Math.floor(rnd() * fnIds.length);
      if (j === i) continue;
      const r = rnd();
      if (r < 0.02)
        edges.push(
          plain(
            from,
            tables[Math.floor(rnd() * tables.length)]!,
            rnd() < 0.5 ? "reads" : "writes",
            "heuristic",
          ),
        );
      else if (r < 0.03)
        edges.push(
          plain(from, routes[Math.floor(rnd() * routes.length)]!, "requests", "heuristic"),
        );
      else if (r < 0.035)
        edges.push(plain(from, hosts[Math.floor(rnd() * hosts.length)]!, "requests", "heuristic"));
      else
        edges.push(
          plain(from, fnIds[j]!, rnd() < 0.05 ? "spawns" : "calls", rnd() < 0.7 ? "exact" : "name"),
        );
    }
  }
  for (const route of routes) {
    edges.push(plain(route, fnIds[Math.floor(rnd() * fnIds.length)]!, "handles"));
  }
  return {
    meta: {
      version: 1,
      root: "/synthetic",
      generated_at: new Date(0).toISOString(),
      languages: LANGS,
      files: fileIds.length,
      warnings: [],
    },
    nodes,
    edges,
  };
}

function plain(
  from: string,
  to: string,
  kind: Edge["kind"],
  confidence: Edge["confidence"] = "exact",
): Edge {
  return {
    from,
    to,
    kind,
    confidence,
    line: null,
    loop_depth: 0,
    conditional: false,
    awaited: false,
    attrs: {},
  };
}
