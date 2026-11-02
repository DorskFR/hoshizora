import { apiBase } from "./router";
import type {
  AddProject,
  Graph,
  GraphNode,
  Impact,
  Insights,
  LiveEvent,
  NodeDetail,
  NodeStats,
  Project,
  Purged,
  PurgeReport,
  SearchHit,
  Storage,
  Story,
  Subgraph,
  TraceSummary,
} from "./types";

export interface Backend {
  graph(): Promise<Graph>;
  node(id: string): Promise<NodeDetail>;
  search(q: string, limit?: number): Promise<SearchHit[]>;
  entrypoints(): Promise<GraphNode[]>;
  flow(from: string, depth?: number): Promise<Subgraph>;
  /** `null` when the server predates `/api/story`. */
  story(from: string): Promise<Story | null>;
  impact(of: string, depth?: number): Promise<Impact>;
  insights(): Promise<Insights>;
  runtimeStats(): Promise<NodeStats[]>;
  traces(limit?: number): Promise<TraceSummary[]>;
  trace(id: string): Promise<TraceSummary>;
  stream(onEvent: (ev: LiveEvent) => void, onState?: (open: boolean) => void): () => void;
}

export interface ProjectsApi {
  /** `null` when the server has a single unnamed project and no `/api/projects`. */
  list(): Promise<Project[] | null>;
  get(id: string): Promise<Project>;
  add(body: AddProject): Promise<Project>;
  reindex(id: string): Promise<Project>;
  remove(id: string): Promise<void>;
  /** Deletes the clone, keeps the project and its graph; reindex clones again. */
  purge(id: string): Promise<Purged>;
  purgeAll(): Promise<PurgeReport>;
  /** `null` when the server predates `/api/storage`. */
  storage(): Promise<Storage | null>;
  backend(id: string | null): Backend;
}

export class HttpError extends Error {
  constructor(
    readonly url: string,
    readonly status: number,
    message: string,
  ) {
    super(message);
  }
}

/** Old servers answer unknown `/api` paths with the SPA's index.html, so a non-JSON body counts as missing. */
async function request<T>(url: string, init?: RequestInit): Promise<T> {
  const res = await fetch(url, {
    ...init,
    headers: { accept: "application/json", ...init?.headers },
  });
  if (!res.ok) {
    let detail = "";
    try {
      detail = (await res.text()).slice(0, 300);
    } catch {
      /* body unavailable */
    }
    throw new HttpError(
      url,
      res.status,
      `${url}: HTTP ${res.status}${detail ? ` · ${detail}` : ""}`,
    );
  }
  if (res.status === 204) return undefined as T;
  const type = res.headers.get("content-type") ?? "";
  if (!type.includes("json")) throw new HttpError(url, 404, `${url}: not a JSON endpoint`);
  return (await res.json()) as T;
}

export class HttpBackend implements Backend {
  constructor(private readonly base = "/api") {}

  graph() {
    return request<Graph>(`${this.base}/graph?detail=slim`);
  }
  node(id: string) {
    return request<NodeDetail>(`${this.base}/nodes/${encodeURIComponent(id)}`);
  }
  search(q: string, limit = 20) {
    return request<SearchHit[]>(`${this.base}/search?q=${encodeURIComponent(q)}&limit=${limit}`);
  }
  entrypoints() {
    return request<GraphNode[]>(`${this.base}/entrypoints`);
  }
  flow(from: string, depth = 12) {
    return request<Subgraph>(`${this.base}/flow?from=${encodeURIComponent(from)}&depth=${depth}`);
  }
  async story(from: string) {
    try {
      return await request<Story>(`${this.base}/story?from=${encodeURIComponent(from)}`);
    } catch (err) {
      if (err instanceof HttpError && err.status === 404) return null;
      throw err;
    }
  }
  impact(of: string, depth = 12) {
    return request<Impact>(`${this.base}/impact?of=${encodeURIComponent(of)}&depth=${depth}`);
  }
  insights() {
    return request<Insights>(`${this.base}/insights`);
  }
  runtimeStats() {
    return request<NodeStats[]>(`${this.base}/runtime/stats`);
  }
  traces(limit = 50) {
    return request<TraceSummary[]>(`${this.base}/runtime/traces?limit=${limit}`);
  }
  trace(id: string) {
    return request<TraceSummary>(`${this.base}/runtime/traces/${encodeURIComponent(id)}`);
  }
  stream(onEvent: (ev: LiveEvent) => void, onState?: (open: boolean) => void) {
    const es = new EventSource(`${this.base}/runtime/stream`);
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

export class HttpProjects implements ProjectsApi {
  async list() {
    try {
      return await request<Project[]>("/api/projects");
    } catch (err) {
      if (err instanceof HttpError && (err.status === 404 || err.status === 405)) return null;
      throw err;
    }
  }
  get(id: string) {
    return request<Project>(`/api/projects/${encodeURIComponent(id)}`);
  }
  add(body: AddProject) {
    return request<Project>("/api/projects", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(body),
    });
  }
  reindex(id: string) {
    return request<Project>(`/api/projects/${encodeURIComponent(id)}/reindex`, { method: "POST" });
  }
  remove(id: string) {
    return request<void>(`/api/projects/${encodeURIComponent(id)}`, { method: "DELETE" });
  }
  purge(id: string) {
    return request<Purged>(`/api/projects/${encodeURIComponent(id)}/purge`, { method: "POST" });
  }
  purgeAll() {
    return request<PurgeReport>("/api/purge", { method: "POST" });
  }
  async storage() {
    try {
      return await request<Storage>("/api/storage");
    } catch (err) {
      if (err instanceof HttpError && (err.status === 404 || err.status === 405)) return null;
      throw err;
    }
  }
  backend(id: string | null) {
    return new HttpBackend(apiBase(id));
  }
}

export function isMockMode(): boolean {
  const params = new URLSearchParams(location.search);
  return params.has("mock") || params.has("synthetic") || import.meta.env.VITE_MOCK === "1";
}

export async function createProjects(): Promise<ProjectsApi> {
  if (!isMockMode()) return new HttpProjects();
  const { MockProjects } = await import("./mock/backend");
  const params = new URLSearchParams(location.search);
  const synthetic = params.get("synthetic");
  return new MockProjects(synthetic ? Number(synthetic) || 20000 : null);
}
