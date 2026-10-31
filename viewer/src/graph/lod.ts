import type { Confidence, EdgeKind, EffectKind, EntrypointKind, NodeKind } from "../types";
import { SYNTHETIC_KINDS } from "../types";
import type { GraphIndex } from "./query";

export type Level = "packages" | "files" | "functions";

export interface ViewNode {
  id: string;
  label: string;
  kind: NodeKind;
  aggregate: boolean;
  members: string[];
  lines: number;
  pkg: string | null;
  pkgIndex: number;
  effect: EffectKind | null;
  effects: EffectKind[];
  dead: boolean;
  entrypoint: EntrypointKind | null;
  synthetic: boolean;
  val: number;
  x?: number;
  y?: number;
  z?: number;
}

export interface ViewLink {
  id: string;
  source: string;
  target: string;
  kind: EdgeKind;
  confidence: Confidence;
  weight: number;
}

export interface LodOptions {
  level: Level;
  expanded: ReadonlySet<string>;
  maxLinks: number;
  effectFilter: ReadonlySet<EffectKind>;
  transitive: boolean;
}

export interface View {
  nodes: ViewNode[];
  links: ViewLink[];
  repOf: Map<string, string>;
  dropped: number;
}

const LINK_BOOST: Partial<Record<EdgeKind, number>> = {
  handles: 3,
  requests: 3,
  spawns: 2,
  writes: 2,
  reads: 2,
  publishes: 2,
  subscribes: 2,
};

function linkScore(l: ViewLink): number {
  return l.weight * (LINK_BOOST[l.kind] ?? 1);
}

/** Maps a node to the node that stands for it at the current level, or `null` when it is an opened container. */
export function representative(
  index: GraphIndex,
  id: string,
  level: Level,
  expanded: ReadonlySet<string>,
): string | null {
  const n = index.node(id);
  if (!n) return null;
  if (SYNTHETIC_KINDS.has(n.kind)) return id;
  const pkg = n.kind === "package" ? n.id : index.packageOf(id);
  const pkgOpen = level !== "packages" || (pkg !== null && expanded.has(pkg));
  if (!pkgOpen) return pkg ?? id;
  if (n.kind === "package") return null;
  const file = n.kind === "file" ? n.id : index.fileOf(id);
  const fileOpen = level === "functions" || (file !== null && expanded.has(file));
  if (!fileOpen) return file ?? id;
  if (n.kind === "file") return null;
  return id;
}

export function buildView(index: GraphIndex, opts: LodOptions): View {
  const repOf = new Map<string, string>();
  const groups = new Map<string, string[]>();
  for (const n of index.graph.nodes) {
    const rep = representative(index, n.id, opts.level, opts.expanded);
    if (!rep) continue;
    repOf.set(n.id, rep);
    const g = groups.get(rep);
    if (g) g.push(n.id);
    else groups.set(rep, [n.id]);
  }

  const packages = index.graph.nodes
    .filter((n) => n.kind === "package")
    .map((n) => n.id)
    .sort();
  const pkgIndex = new Map(packages.map((id, i) => [id, i]));

  const nodes: ViewNode[] = [];
  for (const [rep, members] of groups) {
    const base = index.node(rep)!;
    const counts = new Map<EffectKind, number>();
    let lines = 0;
    let dead = members.length > 0;
    let entrypoint: EntrypointKind | null = base.entrypoint;
    for (const id of members) {
      const m = index.node(id)!;
      if (m.kind === "function" || m.kind === "type") lines += m.lines;
      if (!m.dead && (m.kind === "function" || m.kind === "type")) dead = false;
      if (!entrypoint && m.entrypoint) entrypoint = m.entrypoint;
      const kinds = opts.transitive ? m.effects.transitive : m.effects.direct.map((e) => e.kind);
      for (const k of kinds) {
        if (!opts.effectFilter.has(k)) continue;
        counts.set(k, (counts.get(k) ?? 0) + 1);
      }
    }
    if (lines === 0) lines = base.lines;
    if (
      base.kind !== "function" &&
      base.kind !== "type" &&
      members.every((id) => index.node(id)!.kind !== "function")
    ) {
      dead = false;
    }
    const aggregate = base.kind === "package" || base.kind === "file";
    const pkg = SYNTHETIC_KINDS.has(base.kind)
      ? null
      : base.kind === "package"
        ? base.id
        : index.packageOf(rep);
    const effects = Array.from(counts.entries())
      .sort((a, b) => b[1] - a[1])
      .map(([k]) => k);
    nodes.push({
      id: rep,
      label: base.name,
      kind: base.kind,
      aggregate,
      members,
      lines,
      pkg,
      pkgIndex: pkg ? (pkgIndex.get(pkg) ?? 0) : -1,
      effect: effects[0] ?? null,
      effects,
      dead,
      entrypoint,
      synthetic: SYNTHETIC_KINDS.has(base.kind),
      val: nodeValue(base.kind, lines, members.length),
    });
  }

  const merged = new Map<string, ViewLink>();
  for (const e of index.graph.edges) {
    if (e.kind === "contains") continue;
    const a = repOf.get(e.from);
    const b = repOf.get(e.to);
    if (!a || !b || a === b) continue;
    const key = `${a}|${b}|${e.kind}`;
    const cur = merged.get(key);
    if (cur) {
      cur.weight += 1;
      if (rankConfidence(e.confidence) > rankConfidence(cur.confidence))
        cur.confidence = e.confidence;
    } else {
      merged.set(key, {
        id: key,
        source: a,
        target: b,
        kind: e.kind,
        confidence: e.confidence,
        weight: 1,
      });
    }
  }
  const all = Array.from(merged.values()).sort((x, y) => linkScore(y) - linkScore(x));
  const links = all.slice(0, opts.maxLinks);
  return { nodes, links, repOf, dropped: all.length - links.length };
}

function rankConfidence(c: Confidence): number {
  return c === "observed" ? 3 : c === "exact" ? 2 : c === "name" ? 1 : 0;
}

export function nodeValue(kind: NodeKind, lines: number, members: number): number {
  if (SYNTHETIC_KINDS.has(kind)) return 3;
  if (kind === "package") return Math.max(6, Math.cbrt(lines + 1) * 2 + Math.sqrt(members));
  if (kind === "file") return Math.max(3, Math.cbrt(lines + 1) * 1.2 + Math.sqrt(members) * 0.5);
  return Math.max(0.6, Math.cbrt(lines + 1) * 0.6);
}

/** Containers (package at `packages`, file at `files`) that must be opened to reveal `ids` at the current level. */
export function containersToExpand(
  index: GraphIndex,
  ids: Iterable<string>,
  level: Level,
): Set<string> {
  const out = new Set<string>();
  for (const id of ids) {
    const n = index.node(id);
    if (!n || SYNTHETIC_KINDS.has(n.kind)) continue;
    if (level === "packages") {
      const pkg = index.packageOf(id);
      if (pkg && pkg !== id) out.add(pkg);
    }
    if (level !== "functions" && (n.kind === "function" || n.kind === "type")) {
      const file = index.fileOf(id);
      if (file) out.add(file);
    }
  }
  return out;
}
