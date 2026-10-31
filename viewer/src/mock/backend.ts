import {
  buildTrace,
  demoFlowSpans,
  demoGraph,
  demoInsights,
  demoSources,
  demoStats,
} from "../../fixtures/demo";
import { syntheticGraph } from "../../fixtures/synthetic";
import type { Backend } from "../api";
import { flowFrom, GraphIndex, impactOf } from "../graph/query";
import type {
  Graph,
  GraphNode,
  LiveEvent,
  NodeDetail,
  NodeStats,
  SearchHit,
  TraceSummary,
} from "../types";

const MS = 1_000_000;

export class MockBackend implements Backend {
  private readonly g: Graph;
  private readonly index: GraphIndex;
  private readonly recent: TraceSummary[] = [];
  private readonly synthetic: boolean;
  private statsCache: NodeStats[];

  constructor(syntheticFunctions: number | null) {
    this.synthetic = syntheticFunctions !== null;
    this.g = this.synthetic ? syntheticGraph({ functions: syntheticFunctions! }) : demoGraph;
    this.index = new GraphIndex(this.g);
    this.statsCache = this.synthetic ? [] : demoStats();
    if (!this.synthetic) {
      const now = Date.now() * MS;
      for (let i = 6; i >= 1; i--) {
        this.recent.push(
          buildTrace(`t${String(i).padStart(4, "0")}`, now - i * 9_000 * MS, 0.8 + (i % 3) * 0.2),
        );
      }
    }
  }

  async graph() {
    return this.g;
  }

  async node(id: string): Promise<NodeDetail> {
    const node = this.index.node(id);
    if (!node) throw new Error(`unknown node ${id}`);
    let source: NodeDetail["source"] = null;
    if (node.file && node.span && demoSources[node.file]) {
      const lines = demoSources[node.file]!.split("\n");
      source = {
        file: node.file,
        start_line: node.span.start_line,
        end_line: node.span.end_line,
        text: lines.slice(node.span.start_line - 1, node.span.end_line).join("\n"),
      };
    }
    return {
      node,
      source,
      incoming: this.index.in.get(id) ?? [],
      outgoing: this.index.out.get(id) ?? [],
      stats: this.statsCache.find((s) => s.node === id) ?? null,
    };
  }

  async search(q: string, limit = 20): Promise<SearchHit[]> {
    const needle = q.toLowerCase();
    if (!needle) return [];
    const hits: SearchHit[] = [];
    for (const n of this.g.nodes) {
      const name = n.name.toLowerCase();
      const qn = n.qualified_name.toLowerCase();
      let score = 0;
      if (name === needle) score = 1;
      else if (name.startsWith(needle)) score = 0.8;
      else if (name.includes(needle)) score = 0.6;
      else if (qn.includes(needle)) score = 0.4;
      if (score > 0)
        hits.push({
          node: n.id,
          name: n.name,
          qualified_name: n.qualified_name,
          kind: n.kind,
          score,
        });
      if (hits.length > limit * 4) break;
    }
    return hits.sort((a, b) => b.score - a.score).slice(0, limit);
  }

  async entrypoints(): Promise<GraphNode[]> {
    return this.g.nodes.filter((n) => n.entrypoint);
  }

  async flow(from: string, depth = 12) {
    return flowFrom(this.index, from, depth);
  }

  async impact(of: string, depth = 12) {
    return impactOf(this.index, of, depth);
  }

  async insights() {
    return { insights: this.synthetic ? [] : demoInsights };
  }

  async runtimeStats() {
    return this.statsCache;
  }

  async traces() {
    return [...this.recent].reverse();
  }

  async trace(id: string) {
    const t = this.recent.find((t) => t.trace_id === id);
    if (!t) throw new Error(`unknown trace ${id}`);
    return t;
  }

  stream(onEvent: (ev: LiveEvent) => void, onState?: (open: boolean) => void): () => void {
    if (this.synthetic) {
      onState?.(true);
      return () => onState?.(false);
    }
    let seq = this.recent.length;
    const timers: ReturnType<typeof setTimeout>[] = [];
    const emitTrace = () => {
      seq += 1;
      const id = `t${String(seq).padStart(4, "0")}`;
      const jitter = 0.7 + Math.random() * 0.8;
      const trace = buildTrace(id, Date.now() * MS, jitter);
      this.recent.push(trace);
      if (this.recent.length > 50) this.recent.shift();
      for (const [i, span] of trace.spans.entries()) {
        const spec = demoFlowSpans[i]!;
        timers.push(setTimeout(() => onEvent({ type: "span", span }), spec.end * jitter));
      }
      timers.push(
        setTimeout(() => {
          this.statsCache = demoStats(0.8 + Math.random() * 0.6);
          onEvent({ type: "stats", stats: this.statsCache });
        }, 500 * jitter),
      );
    };
    onState?.(true);
    emitTrace();
    const interval = setInterval(emitTrace, 2000);
    return () => {
      clearInterval(interval);
      for (const t of timers) clearTimeout(t);
      onState?.(false);
    };
  }
}
