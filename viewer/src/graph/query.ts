import type { Edge, EdgeKind, FlowStep, Graph, GraphNode, Impact, Subgraph } from "../types";

export const FLOW_KINDS: ReadonlySet<EdgeKind> = new Set<EdgeKind>([
  "calls",
  "spawns",
  "requests",
  "handles",
  "reads",
  "writes",
  "publishes",
]);

export class GraphIndex {
  readonly byId = new Map<string, GraphNode>();
  readonly out = new Map<string, Edge[]>();
  readonly in = new Map<string, Edge[]>();
  readonly children = new Map<string, string[]>();

  constructor(readonly graph: Graph) {
    for (const n of graph.nodes) {
      this.byId.set(n.id, n);
      if (n.parent) push(this.children, n.parent, n.id);
    }
    for (const e of graph.edges) {
      push(this.out, e.from, e);
      push(this.in, e.to, e);
    }
  }

  node(id: string): GraphNode | undefined {
    return this.byId.get(id);
  }

  /** Walks `parent` links up to the package (or the topmost ancestor). */
  packageOf(id: string): string | null {
    let cur = this.byId.get(id);
    let last: GraphNode | undefined;
    while (cur) {
      last = cur;
      if (cur.kind === "package") return cur.id;
      cur = cur.parent ? this.byId.get(cur.parent) : undefined;
    }
    return last && last.kind !== "function" && last.kind !== "type" ? last.id : null;
  }

  fileOf(id: string): string | null {
    let cur = this.byId.get(id);
    while (cur) {
      if (cur.kind === "file") return cur.id;
      cur = cur.parent ? this.byId.get(cur.parent) : undefined;
    }
    return null;
  }

  lane(id: string): string {
    const n = this.byId.get(id);
    if (!n) return "?";
    if (n.kind === "route" || n.kind === "host" || n.kind === "topic") return "network";
    if (n.kind === "table") return "database";
    if (n.kind === "dependency") return "dependencies";
    const pkg = this.packageOf(id);
    return pkg ? (this.byId.get(pkg)?.name ?? pkg) : (n.language ?? "?");
  }
}

function push<K, V>(m: Map<K, V[]>, k: K, v: V) {
  const arr = m.get(k);
  if (arr) arr.push(v);
  else m.set(k, [v]);
}

/** Depth-first forward reachability, mirroring `/api/flow`. */
export function flowFrom(
  index: GraphIndex,
  from: string,
  depth = 12,
  kinds = FLOW_KINDS,
): Subgraph {
  const visited = new Set<string>([from]);
  const steps: FlowStep[] = [];
  const edges: Edge[] = [];
  let truncated = false;
  const visit = (id: string, d: number, via: EdgeKind | null) => {
    steps.push({ node: id, depth: d, via, lane: index.lane(id) });
    if (d >= depth) {
      if ((index.out.get(id) ?? []).some((e) => kinds.has(e.kind))) truncated = true;
      return;
    }
    for (const e of index.out.get(id) ?? []) {
      if (!kinds.has(e.kind)) continue;
      edges.push(e);
      if (visited.has(e.to)) continue;
      visited.add(e.to);
      visit(e.to, d + 1, e.kind);
    }
  };
  visit(from, 0, null);
  return { root: from, nodes: Array.from(visited), edges, steps, truncated };
}

/** Reverse reachability with shortest paths from each entrypoint, mirroring `/api/impact`. */
export function impactOf(index: GraphIndex, of: string, depth = 12, kinds = FLOW_KINDS): Impact {
  const next = new Map<string, string>();
  const dist = new Map<string, number>([[of, 0]]);
  const queue = [of];
  const edges: Edge[] = [];
  while (queue.length) {
    const id = queue.shift()!;
    const d = dist.get(id)!;
    if (d >= depth) continue;
    for (const e of index.in.get(id) ?? []) {
      if (!kinds.has(e.kind)) continue;
      edges.push(e);
      if (dist.has(e.from)) continue;
      dist.set(e.from, d + 1);
      next.set(e.from, id);
      queue.push(e.from);
    }
  }
  const affected = Array.from(dist.keys()).filter((id) => id !== of);
  const entrypoints = affected
    .map((id) => index.node(id))
    .filter((n): n is GraphNode => !!n?.entrypoint)
    .map((n) => {
      const path = [n.id];
      let cur = n.id;
      while (cur !== of) {
        cur = next.get(cur)!;
        path.push(cur);
      }
      return { node: n.id, kind: n.entrypoint!, path };
    });
  return { target: of, affected, entrypoints, edges };
}
