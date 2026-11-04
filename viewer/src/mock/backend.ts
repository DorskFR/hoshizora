import { cctuiGraph, cctuiInsights, cctuiSources, cctuiSpawnStory } from "../../fixtures/cctui";
import {
  buildTrace,
  demoFlowSpans,
  demoGraph,
  demoInsights,
  demoSources,
  demoStats,
} from "../../fixtures/demo";
import { syntheticGraph } from "../../fixtures/synthetic";
import type { Backend, ProjectsApi } from "../api";
import { flowFrom, GraphIndex, impactOf } from "../graph/query";
import { storyFromFlow } from "../story/build";
import type {
  AddProject,
  Graph,
  GraphNode,
  Insight,
  LiveEvent,
  NodeDetail,
  NodeStats,
  Project,
  Purged,
  SearchHit,
  Story,
  TraceSummary,
  UpdateProject,
} from "../types";

const MS = 1_000_000;

export interface MockDataset {
  graph: Graph;
  sources: Record<string, string>;
  insights: Insight[];
  stories: Story[];
  live: boolean;
}

export const DATASETS = {
  cctui: (): MockDataset => ({
    graph: cctuiGraph,
    sources: cctuiSources,
    insights: cctuiInsights,
    stories: [cctuiSpawnStory],
    live: false,
  }),
  shop: (): MockDataset => ({
    graph: demoGraph,
    sources: demoSources,
    insights: demoInsights,
    stories: [],
    live: true,
  }),
  synthetic: (functions: number): MockDataset => ({
    graph: syntheticGraph({ functions }),
    sources: {},
    insights: [],
    stories: [],
    live: false,
  }),
};

export class MockBackend implements Backend {
  private readonly g: Graph;
  private readonly index: GraphIndex;
  private readonly recent: TraceSummary[] = [];
  private statsCache: NodeStats[];

  constructor(private readonly data: MockDataset) {
    this.g = data.graph;
    this.index = new GraphIndex(this.g);
    this.statsCache = data.live ? demoStats() : [];
    if (data.live) {
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
    const text = node.file ? this.data.sources[node.file] : undefined;
    if (node.file && node.span && text) {
      const lines = text.split("\n");
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
    const needle = q.toLowerCase().replace(/[\s_-]/g, "");
    if (!needle) return [];
    const hits: SearchHit[] = [];
    for (const n of this.g.nodes) {
      const name = n.name.toLowerCase().replace(/[_-]/g, "");
      const qn = n.qualified_name.toLowerCase().replace(/[_-]/g, "");
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

  async story(from: string): Promise<Story | null> {
    const fixed = this.data.stories.find((s) => s.root === from);
    if (fixed) return fixed;
    return storyFromFlow(this.index, flowFrom(this.index, from));
  }

  async impact(of: string, depth = 12) {
    return impactOf(this.index, of, depth);
  }

  async insights() {
    return { insights: this.data.insights };
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
    if (!this.data.live) {
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

interface MockProject {
  project: Project;
  dataset: () => MockDataset;
  cloneBytes: number | null;
}

const STAGE_MS = 1800;

/** In-memory projects; added repositories walk through cloning → indexing → ready on a timer. */
export class MockProjects implements ProjectsApi {
  private readonly projects = new Map<string, MockProject>();

  constructor(private readonly syntheticFunctions: number | null) {
    this.put(
      {
        id: "dorskfr-cctui",
        name: "cctui",
        source: { kind: "git", url: "https://github.com/DorskFR/cctui", ref: "main" },
        status: "ready",
        error: null,
        commit: "4f2c9e1a7b3d5f60c8e2a1b9d7f3e5c2a4b6d8e0",
        indexed_at: "2026-10-31T08:17:31Z",
        nodes: cctuiGraph.nodes.length,
        edges: cctuiGraph.edges.length,
        services: ["cctui-server", "cctui-daemon"],
      },
      DATASETS.cctui,
    );
    this.put(
      {
        id: "shop",
        name: "shop",
        source: { kind: "path", path: "/home/dev/shop" },
        status: "ready",
        error: null,
        commit: null,
        indexed_at: "2026-10-30T18:02:11Z",
        nodes: demoGraph.nodes.length,
        edges: demoGraph.edges.length,
        services: ["api", "worker"],
      },
      DATASETS.shop,
    );
  }

  private put(project: Project, dataset: () => MockDataset): void {
    const cloneBytes = project.source.kind === "git" ? 48_000_000 + project.nodes * 2_000 : null;
    this.projects.set(project.id, { project, dataset, cloneBytes });
  }

  async list() {
    if (this.syntheticFunctions !== null) return null;
    return Array.from(this.projects.values()).map((p) => ({ ...p.project }));
  }

  async get(id: string) {
    const p = this.projects.get(id);
    if (!p) throw new Error(`unknown project ${id}`);
    return { ...p.project };
  }

  async add(body: AddProject) {
    const url = body.url.trim();
    for (const p of this.projects.values())
      if (p.project.source.kind === "git" && p.project.source.url === url) return { ...p.project };
    const slug = url
      .replace(/^https?:\/\/(www\.)?github\.com\//, "")
      .replace(/\.git$/, "")
      .replace(/[^a-zA-Z0-9]+/g, "-")
      .replace(/^-|-$/g, "")
      .toLowerCase();
    const project: Project = {
      id: slug || `repo-${this.projects.size}`,
      name:
        body.name ||
        url
          .split("/")
          .pop()
          ?.replace(/\.git$/, "") ||
        slug,
      source: { kind: "git", url, ref: body.ref ?? null },
      status: "cloning",
      error: null,
      commit: null,
      indexed_at: null,
      nodes: 0,
      edges: 0,
      services: body.services ?? [],
    };
    this.put(project, DATASETS.shop);
    this.advance(project);
    return { ...project };
  }

  private advance(project: Project): void {
    const valid = /^(https?:\/\/|git@|ssh:\/\/)/.test(
      project.source.kind === "git" ? project.source.url : "",
    );
    setTimeout(() => {
      if (!valid) {
        project.status = "failed";
        project.error = "git clone: repository not found or not a git URL";
        return;
      }
      project.status = "indexing";
      project.commit = "9a1b2c3d4e5f60718293a4b5c6d7e8f901234567";
      setTimeout(() => {
        project.status = "ready";
        project.indexed_at = new Date().toISOString();
        project.nodes = demoGraph.nodes.length;
        project.edges = demoGraph.edges.length;
      }, STAGE_MS);
    }, STAGE_MS);
  }

  async reindex(id: string) {
    const p = this.projects.get(id);
    if (!p) throw new Error(`unknown project ${id}`);
    p.project.status = "indexing";
    setTimeout(() => {
      p.project.status = "ready";
      p.project.indexed_at = new Date().toISOString();
      if (p.project.source.kind === "git") p.cloneBytes ??= 48_000_000;
    }, STAGE_MS);
    return { ...p.project };
  }

  async update(id: string, body: UpdateProject) {
    const p = this.projects.get(id);
    if (!p) throw new Error(`unknown project ${id}`);
    if (body.auto_refresh !== undefined) {
      if (body.auto_refresh && p.project.source.kind !== "git")
        throw new Error("a path project has no remote to refresh from");
      p.project.auto_refresh = body.auto_refresh;
    }
    return { ...p.project };
  }

  async remove(id: string) {
    this.projects.delete(id);
  }

  async purge(id: string) {
    const p = this.projects.get(id);
    if (!p) throw new Error(`unknown project ${id}`);
    if (p.project.source.kind !== "git") throw new Error("a path project has no clone to purge");
    const freed = p.cloneBytes;
    p.cloneBytes = null;
    return { id, freed_bytes: freed ?? 0 };
  }

  async purgeAll() {
    const projects: Purged[] = [];
    for (const p of this.projects.values()) {
      if (p.project.source.kind !== "git" || p.cloneBytes === null) continue;
      projects.push(await this.purge(p.project.id));
    }
    return { projects, freed_bytes: projects.reduce((n, p) => n + p.freed_bytes, 0) };
  }

  async storage() {
    const projects = [...this.projects.values()].map((p) => ({
      id: p.project.id,
      clone_bytes: p.cloneBytes,
      graph_bytes: p.project.nodes * 900 + p.project.edges * 300,
    }));
    return {
      projects,
      clone_bytes: projects.reduce((n, p) => n + (p.clone_bytes ?? 0), 0),
      graph_bytes: projects.reduce((n, p) => n + p.graph_bytes, 0),
      disk: { total_bytes: 1_073_741_824, free_bytes: 734_003_200 },
    };
  }

  backend(id: string | null): Backend {
    if (this.syntheticFunctions !== null)
      return new MockBackend(DATASETS.synthetic(this.syntheticFunctions));
    const p = id ? this.projects.get(id) : this.projects.values().next().value;
    if (!p) throw new Error(`unknown project ${id}`);
    return new MockBackend(p.dataset());
  }
}
