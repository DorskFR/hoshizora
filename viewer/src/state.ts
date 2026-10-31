import type { Backend } from "./api";
import type { Level } from "./graph/lod";
import type { GraphIndex } from "./graph/query";
import type {
  EffectKind,
  Graph,
  Impact,
  NodeKind,
  NodeStats,
  Subgraph,
  TraceSummary,
} from "./types";
import { EFFECT_KINDS } from "./types";

export type Mode = "explore" | "flow" | "impact";
export type HeatMetric = "off" | "self" | "rate" | "p95";

export interface Events {
  view: undefined;
  selection: string | null;
  mode: Mode;
  runtime: undefined;
  stats: undefined;
  heat: undefined;
  status: string;
  traces: TraceSummary[];
}

type Handler<T> = (payload: T) => void;

export class Store {
  graph!: Graph;
  index!: GraphIndex;
  level: Level = "packages";
  expanded = new Set<string>();
  effectFilter = new Set<EffectKind>(EFFECT_KINDS);
  transitive = false;
  hideKinds = new Set<NodeKind>(["dependency"]);
  maxLinks = 6000;
  selected: string | null = null;
  mode: Mode = "explore";
  flow: Subgraph | null = null;
  impact: Impact | null = null;
  runtimeOn = false;
  heat: HeatMetric = "off";
  stats = new Map<string, NodeStats>();
  traces: TraceSummary[] = [];
  private handlers = new Map<keyof Events, Set<Handler<unknown>>>();

  constructor(readonly backend: Backend) {}

  on<K extends keyof Events>(event: K, fn: Handler<Events[K]>): () => void {
    let set = this.handlers.get(event);
    if (!set) {
      set = new Set();
      this.handlers.set(event, set);
    }
    set.add(fn as Handler<unknown>);
    return () => set?.delete(fn as Handler<unknown>);
  }

  emit<K extends keyof Events>(event: K, payload: Events[K]): void {
    for (const fn of this.handlers.get(event) ?? []) fn(payload);
  }

  select(id: string | null): void {
    this.selected = id;
    this.emit("selection", id);
  }

  setMode(mode: Mode): void {
    this.mode = mode;
    if (mode !== "flow") this.flow = null;
    if (mode !== "impact") this.impact = null;
    this.emit("mode", mode);
  }

  setLevel(level: Level): void {
    this.level = level;
    this.expanded.clear();
    this.emit("view", undefined);
  }

  toggleExpanded(id: string): void {
    if (this.expanded.has(id)) this.expanded.delete(id);
    else this.expanded.add(id);
    this.emit("view", undefined);
  }

  expandAll(ids: Iterable<string>): void {
    for (const id of ids) this.expanded.add(id);
    this.emit("view", undefined);
  }

  status(msg: string): void {
    this.emit("status", msg);
  }
}
