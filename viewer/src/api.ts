import type {
  Graph,
  GraphNode,
  Impact,
  Insights,
  LiveEvent,
  NodeDetail,
  NodeStats,
  SearchHit,
  Subgraph,
  TraceSummary,
} from "./types";

export interface Backend {
  graph(): Promise<Graph>;
  node(id: string): Promise<NodeDetail>;
  search(q: string, limit?: number): Promise<SearchHit[]>;
  entrypoints(): Promise<GraphNode[]>;
  flow(from: string, depth?: number): Promise<Subgraph>;
  impact(of: string, depth?: number): Promise<Impact>;
  insights(): Promise<Insights>;
  runtimeStats(): Promise<NodeStats[]>;
  traces(limit?: number): Promise<TraceSummary[]>;
  trace(id: string): Promise<TraceSummary>;
  stream(onEvent: (ev: LiveEvent) => void, onState?: (open: boolean) => void): () => void;
}

async function getJson<T>(url: string): Promise<T> {
  const res = await fetch(url, { headers: { accept: "application/json" } });
  if (!res.ok) throw new Error(`${url}: HTTP ${res.status}`);
  return (await res.json()) as T;
}

export class HttpBackend implements Backend {
  constructor(private readonly base = "") {}

  graph() {
    return getJson<Graph>(`${this.base}/api/graph`);
  }
  node(id: string) {
    return getJson<NodeDetail>(`${this.base}/api/nodes/${encodeURIComponent(id)}`);
  }
  search(q: string, limit = 20) {
    return getJson<SearchHit[]>(
      `${this.base}/api/search?q=${encodeURIComponent(q)}&limit=${limit}`,
    );
  }
  entrypoints() {
    return getJson<GraphNode[]>(`${this.base}/api/entrypoints`);
  }
  flow(from: string, depth = 12) {
    return getJson<Subgraph>(
      `${this.base}/api/flow?from=${encodeURIComponent(from)}&depth=${depth}`,
    );
  }
  impact(of: string, depth = 12) {
    return getJson<Impact>(`${this.base}/api/impact?of=${encodeURIComponent(of)}&depth=${depth}`);
  }
  insights() {
    return getJson<Insights>(`${this.base}/api/insights`);
  }
  runtimeStats() {
    return getJson<NodeStats[]>(`${this.base}/api/runtime/stats`);
  }
  traces(limit = 50) {
    return getJson<TraceSummary[]>(`${this.base}/api/runtime/traces?limit=${limit}`);
  }
  trace(id: string) {
    return getJson<TraceSummary>(`${this.base}/api/runtime/traces/${encodeURIComponent(id)}`);
  }
  stream(onEvent: (ev: LiveEvent) => void, onState?: (open: boolean) => void) {
    const es = new EventSource(`${this.base}/api/runtime/stream`);
    es.onopen = () => onState?.(true);
    es.onerror = () => onState?.(false);
    es.onmessage = (m) => {
      try {
        onEvent(JSON.parse(m.data) as LiveEvent);
      } catch {
        /* skip malformed frames */
      }
    };
    return () => es.close();
  }
}

export function isMockMode(): boolean {
  const params = new URLSearchParams(location.search);
  return params.has("mock") || params.has("synthetic") || import.meta.env.VITE_MOCK === "1";
}

export async function createBackend(): Promise<Backend> {
  if (!isMockMode()) return new HttpBackend();
  const { MockBackend } = await import("./mock/backend");
  const params = new URLSearchParams(location.search);
  const synthetic = params.get("synthetic");
  return new MockBackend(synthetic ? Number(synthetic) || 20000 : null);
}
