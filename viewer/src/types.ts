export type Language = "rust" | "python" | "typescript" | "javascript" | "go";

export interface Span {
  start_line: number;
  end_line: number;
}

export interface Param {
  name: string;
  ty: string | null;
  default: string | null;
}

export type NodeKind =
  | "package"
  | "file"
  | "type"
  | "function"
  | "route"
  | "table"
  | "host"
  | "topic"
  | "dependency";

export const SYNTHETIC_KINDS: ReadonlySet<NodeKind> = new Set<NodeKind>([
  "route",
  "table",
  "host",
  "topic",
  "dependency",
]);

export type EntrypointKind =
  | "main"
  | "http_handler"
  | "ui_handler"
  | "cli"
  | "job"
  | "test"
  | "export";

export type EffectKind =
  | "network"
  | "db_read"
  | "db_write"
  | "db_delete"
  | "fs_read"
  | "fs_write"
  | "process"
  | "spawn"
  | "join"
  | "lock"
  | "publish"
  | "subscribe"
  | "sleep"
  | "blocking";

export const EFFECT_KINDS: readonly EffectKind[] = [
  "network",
  "db_read",
  "db_write",
  "db_delete",
  "fs_read",
  "fs_write",
  "process",
  "spawn",
  "join",
  "lock",
  "publish",
  "subscribe",
  "sleep",
  "blocking",
];

export interface Effect {
  kind: EffectKind;
  target: string | null;
  via: string;
  line: number;
  loop_depth: number;
  awaited: boolean;
  rule: string;
}

export interface Effects {
  direct: Effect[];
  transitive: EffectKind[];
}

export interface Complexity {
  time: string;
  space: string;
  basis: string[];
}

export interface FunctionInfo {
  signature: string;
  params: Param[];
  returns: string | null;
  doc: string | null;
  is_async: boolean;
  is_method: boolean;
  visibility: "public" | "private";
  attributes: string[];
  cyclomatic: number;
  branches: number;
  loops: number;
  max_loop_depth: number;
  recursive: boolean;
  complexity: Complexity;
  fan_in: number;
  fan_out: number;
}

export interface GraphNode {
  id: string;
  kind: NodeKind;
  name: string;
  qualified_name: string;
  language: Language | null;
  file: string | null;
  span: Span | null;
  parent: string | null;
  function: FunctionInfo | null;
  effects: Effects;
  entrypoint: EntrypointKind | null;
  dead: boolean;
  lines: number;
}

export type EdgeKind =
  | "contains"
  | "calls"
  | "imports"
  | "spawns"
  | "implements"
  | "requests"
  | "handles"
  | "reads"
  | "writes"
  | "publishes"
  | "subscribes"
  | "uses";

export type Confidence = "exact" | "name" | "heuristic" | "observed";

export interface Edge {
  from: string;
  to: string;
  kind: EdgeKind;
  confidence: Confidence;
  line: number | null;
  loop_depth: number;
  conditional: boolean;
  awaited: boolean;
  attrs: Record<string, string>;
}

export interface GraphMeta {
  version: number;
  root: string;
  generated_at: string;
  languages: Language[];
  files: number;
  warnings: string[];
}

export interface Graph {
  meta: GraphMeta;
  nodes: GraphNode[];
  edges: Edge[];
}

export interface Source {
  file: string;
  start_line: number;
  end_line: number;
  text: string;
}

export interface NodeDetail {
  node: GraphNode;
  source: Source | null;
  incoming: Edge[];
  outgoing: Edge[];
  stats: NodeStats | null;
}

export interface FlowStep {
  node: string;
  depth: number;
  via: EdgeKind | null;
  lane: string;
}

export interface Subgraph {
  root: string;
  nodes: string[];
  edges: Edge[];
  steps: FlowStep[];
  truncated: boolean;
}

export interface AffectedEntrypoint {
  node: string;
  kind: EntrypointKind;
  path: string[];
}

export interface Impact {
  target: string;
  affected: string[];
  entrypoints: AffectedEntrypoint[];
  edges: Edge[];
}

export interface SearchHit {
  node: string;
  name: string;
  qualified_name: string;
  kind: NodeKind;
  score: number;
}

export type SpanKind = "internal" | "server" | "client" | "producer" | "consumer";

export interface RuntimeSpan {
  trace_id: string;
  span_id: string;
  parent_span_id: string | null;
  service: string;
  name: string;
  kind: SpanKind;
  start_unix_nanos: number;
  end_unix_nanos: number;
  error: boolean;
  attrs: Record<string, string>;
  node: string | null;
  join: string | null;
}

export interface NodeStats {
  node: string;
  count: number;
  errors: number;
  total_nanos: number;
  self_nanos: number;
  wait_nanos: number;
  p50_nanos: number;
  p95_nanos: number;
  p99_nanos: number;
  max_nanos: number;
  rate_per_sec: number;
  last_seen_unix_nanos: number;
  cpu_nanos: number | null;
  alloc_bytes: number | null;
}

export type Insight =
  | {
      kind: "inefficient";
      node: string;
      self_p95_nanos: number;
      complexity: string;
      reasons: string[];
    }
  | { kind: "hot_path"; node: string; rate_per_sec: number; p95_nanos: number }
  | { kind: "waiting"; node: string; wait_ratio: number; on: string[] }
  | { kind: "parallelisable"; node: string; children: string[]; saved_nanos: number }
  | {
      kind: "io_in_loop";
      node: string;
      effect: string;
      line: number;
      observed_calls_per_parent: number | null;
    }
  | { kind: "never_observed"; node: string }
  | {
      kind: "unhandled_values";
      node: string;
      line: number;
      domain: string;
      missing: string[];
      default: "none" | "throws" | "returns_empty" | "other";
    }
  | {
      kind: "read_then_write";
      node: string;
      table: string;
      read_line: number;
      write_line: number;
      write: "writes" | "deletes";
    };

export interface Insights {
  insights: Insight[];
}

export interface TraceSummary {
  trace_id: string;
  /** Name of the root span. */
  root: string;
  root_node: string | null;
  start_unix_nanos: number;
  duration_nanos: number;
  spans: RuntimeSpan[];
}

export type LiveEvent = { type: "span"; span: RuntimeSpan } | { type: "stats"; stats: NodeStats[] };

export type LaneKind = "ui" | "service" | "worker" | "database" | "queue" | "external";

export interface StoryLane {
  id: string;
  label: string;
  kind: LaneKind;
}

export interface StoryStep {
  node: string;
  lane: string;
  label: string;
  kind: NodeKind;
  seq: number;
  effects: Effect[];
  folded: string[];
}

export interface StoryLink {
  from: string;
  to: string;
  kind: EdgeKind;
  label: string | null;
  seq: number;
  detached: boolean;
  confidence: Confidence;
}

export interface Story {
  root: string;
  lanes: StoryLane[];
  steps: StoryStep[];
  links: StoryLink[];
  truncated: boolean;
}

export type ProjectSource =
  | { kind: "git"; url: string; ref: string | null }
  | { kind: "path"; path: string };

export type ProjectStatus = "cloning" | "indexing" | "ready" | "failed";

export interface Project {
  id: string;
  name: string;
  source: ProjectSource;
  status: ProjectStatus;
  error: string | null;
  commit: string | null;
  indexed_at: string | null;
  nodes: number;
  edges: number;
  services: string[];
}

export interface AddProject {
  url: string;
  ref?: string | null;
  name?: string | null;
  services?: string[];
}

export interface Purged {
  id: string;
  freed_bytes: number;
}

export interface PurgeReport {
  projects: Purged[];
  freed_bytes: number;
}

export interface ProjectStorage {
  id: string;
  /** `null` when the project has no clone (a path project, or a purged one). */
  clone_bytes: number | null;
  graph_bytes: number;
}

export interface Storage {
  projects: ProjectStorage[];
  clone_bytes: number;
  graph_bytes: number;
  disk: { total_bytes: number; free_bytes: number } | null;
}
