import { isTestNode } from "../graph/filters";
import type { GraphIndex } from "../graph/query";
import type {
  Edge,
  Effect,
  EffectKind,
  GraphNode,
  LaneKind,
  Story,
  StoryLane,
  StoryLink,
  StoryStep,
  Subgraph,
} from "../types";

export const MAX_STEPS = 60;

const LANE_ORDER: LaneKind[] = ["ui", "service", "worker", "queue", "database", "external"];

const SIGNIFICANT: ReadonlySet<EffectKind> = new Set<EffectKind>([
  "network",
  "db_read",
  "db_write",
  "fs_write",
  "process",
  "spawn",
  "publish",
  "subscribe",
]);

const UI_PATH =
  /(^|\/)(webui|web|ui|frontend|client|app|site|pages|components)(\/|$)|\.(svelte|vue|tsx|jsx)$/;
const WORKER_NAME = /daemon|worker|agent|job|queue|supervisor|dispatcher|cron|scheduler/i;

export function laneFor(index: GraphIndex, n: GraphNode): StoryLane {
  if (n.kind === "table") return { id: "lane:database", label: "Database", kind: "database" };
  if (n.kind === "topic") return { id: "lane:queue", label: "Queue", kind: "queue" };
  if (n.kind === "host" || n.kind === "dependency" || n.kind === "route")
    return { id: "lane:external", label: "External", kind: "external" };
  const pkgId = index.packageOf(n.id);
  const pkg = pkgId ? index.node(pkgId) : undefined;
  const name = pkg?.name ?? n.language ?? "code";
  const isScript = n.language === "typescript" || n.language === "javascript";
  const kind: LaneKind =
    isScript && n.file && UI_PATH.test(n.file)
      ? "ui"
      : WORKER_NAME.test(name)
        ? "worker"
        : "service";
  return { id: `lane:${pkgId ?? name}`, label: name, kind };
}

function significant(n: GraphNode): boolean {
  return n.effects.direct.some((e) => SIGNIFICANT.has(e.kind));
}

function linkLabel(e: Edge, to: GraphNode | undefined): string | null {
  const name = to?.name ?? e.to;
  switch (e.kind) {
    case "requests":
      return to?.kind === "route" ? name : `request ${name}`;
    case "reads":
      return `read ${name}`;
    case "writes":
      return `write ${name}`;
    case "publishes":
      return `publish ${name}`;
    case "spawns":
      return "spawn";
    default:
      return null;
  }
}

/**
 * Condenses `/api/flow` into a story when the server has no `/api/story`: steps are the root,
 * synthetic nodes, functions with I/O effects and functions in another package than their caller;
 * everything else is folded into the nearest kept caller. Routes become the label of the link from
 * the requester to the handler.
 */
export function storyFromFlow(index: GraphIndex, flow: Subgraph, maxSteps = MAX_STEPS): Story {
  const out = new Map<string, Edge[]>();
  for (const e of flow.edges) {
    if (!trusted(index, e)) continue;
    const list = out.get(e.from);
    if (list) list.push(e);
    else out.set(e.from, [e]);
  }
  for (const list of out.values()) list.sort((a, b) => (a.line ?? 0) - (b.line ?? 0));

  const lanes = new Map<string, StoryLane>();
  const laneOrder: string[] = [];
  const steps = new Map<string, StoryStep>();
  const links: StoryLink[] = [];
  const linkKeys = new Set<string>();
  const visited = new Set<string>();
  let truncated = false;

  const addStep = (n: GraphNode): StoryStep | null => {
    const existing = steps.get(n.id);
    if (existing) return existing;
    if (steps.size >= maxSteps) {
      truncated = true;
      return null;
    }
    const lane = laneFor(index, n);
    if (!lanes.has(lane.id)) {
      lanes.set(lane.id, lane);
      laneOrder.push(lane.id);
    }
    const step: StoryStep = {
      node: n.id,
      lane: lane.id,
      label: stepLabel(n),
      kind: n.kind,
      seq: steps.size,
      effects: [...n.effects.direct],
      folded: [],
    };
    steps.set(n.id, step);
    return step;
  };

  const addLink = (from: string, to: string, e: Edge, label: string | null) => {
    const key = `${from}|${to}`;
    if (linkKeys.has(key) || from === to) return;
    linkKeys.add(key);
    links.push({
      from,
      to,
      kind: e.kind,
      label,
      seq: steps.get(to)?.seq ?? links.length,
      detached: e.kind === "spawns",
      confidence: e.confidence,
    });
  };

  const fold = (anchor: StoryStep, n: GraphNode) => {
    if (anchor.folded.includes(n.id)) return;
    anchor.folded.push(n.id);
    for (const fx of n.effects.direct) if (!hasEffect(anchor.effects, fx)) anchor.effects.push(fx);
  };

  const visit = (id: string, anchor: StoryStep, laneOf: string) => {
    for (const e of out.get(id) ?? []) {
      if (truncated) return;
      let target = index.node(e.to);
      if (!target || isTestNode(target)) continue;
      let edge = e;
      let label = linkLabel(e, target);
      if (target.kind === "route") {
        const handler = (out.get(target.id) ?? []).find((h) => h.kind === "handles");
        const handlerNode = handler ? index.node(handler.to) : undefined;
        if (handler && handlerNode) {
          visited.add(target.id);
          target = handlerNode;
          edge = { ...e, to: handlerNode.id };
          label = index.node(e.to)?.name ?? label;
        }
      }
      if (visited.has(target.id)) {
        if (steps.has(target.id)) addLink(anchor.node, target.id, edge, label);
        continue;
      }
      visited.add(target.id);
      const lane = laneFor(index, target).id;
      const keep =
        target.kind !== "function" ||
        significant(target) ||
        (lane !== laneOf && target.effects.transitive.some((k) => SIGNIFICANT.has(k))) ||
        target.entrypoint === "http_handler" ||
        e.kind !== "calls";
      if (keep) {
        const step = addStep(target);
        if (!step) return;
        addLink(anchor.node, target.id, edge, label);
        visit(target.id, step, lane);
      } else {
        fold(anchor, target);
        visit(target.id, anchor, laneOf);
      }
    }
  };

  const root = index.node(flow.root);
  if (root) {
    visited.add(root.id);
    const step = addStep(root)!;
    visit(root.id, step, laneFor(index, root).id);
  }

  const orderedLanes = laneOrder
    .map((id) => lanes.get(id)!)
    .map((lane, i) => ({ lane, i }))
    .sort((a, b) => LANE_ORDER.indexOf(a.lane.kind) - LANE_ORDER.indexOf(b.lane.kind) || a.i - b.i)
    .map((x) => x.lane);
  return {
    root: flow.root,
    lanes: orderedLanes,
    steps: Array.from(steps.values()),
    links,
    truncated: truncated || flow.truncated,
  };
}

/**
 * Name-matched calls are kept only when they are unambiguous: one candidate, no receiver, and a
 * method only from its own file. Anything looser drags unrelated code into the story.
 */
function trusted(index: GraphIndex, e: Edge): boolean {
  if (e.confidence !== "name") return true;
  if (Number(e.attrs.candidates ?? "1") > 1 || e.attrs.receiver !== undefined) return false;
  const from = index.node(e.from);
  const to = index.node(e.to);
  if (!from || !to) return false;
  if (to.kind === "function" && to.function?.is_method) return from.file === to.file;
  if (/::|\./.test(to.id.slice(to.id.indexOf("#") + 1))) return from.file === to.file;
  return true;
}

function hasEffect(list: Effect[], fx: Effect): boolean {
  return list.some((e) => e.kind === fx.kind && e.target === fx.target);
}

function stepLabel(n: GraphNode): string {
  if (n.kind !== "function" && n.kind !== "type") return n.name;
  const id = n.id.slice(n.id.indexOf("#") + 1);
  return id.includes("::") || id.includes(".") ? id.split(/::/).slice(-2).join("::") : n.name;
}
