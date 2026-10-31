import type { RuntimeSpan } from "../types";

export interface Hop {
  from: string;
  to: string;
  span: RuntimeSpan;
}

interface TraceState {
  nodeOf: Map<string, string | null>;
  parentOf: Map<string, string | null>;
  waiting: Map<string, RuntimeSpan[]>;
}

/**
 * Joins live spans to graph nodes per trace and resolves parent→child hops even when a
 * child span is exported before its parent (the usual case: spans export on end).
 */
export class SpanTracker {
  private readonly traces = new Map<string, TraceState>();

  constructor(private readonly maxTraces = 200) {}

  record(span: RuntimeSpan): Hop[] {
    const t = this.trace(span.trace_id);
    t.nodeOf.set(span.span_id, span.node);
    t.parentOf.set(span.span_id, span.parent_span_id);
    const hops: Hop[] = [];
    if (span.node) {
      const parentNode = span.parent_span_id
        ? this.nearestJoined(t, span.parent_span_id)
        : undefined;
      if (parentNode === undefined && span.parent_span_id) {
        const list = t.waiting.get(span.parent_span_id);
        if (list) list.push(span);
        else t.waiting.set(span.parent_span_id, [span]);
      } else if (parentNode && parentNode !== span.node) {
        hops.push({ from: parentNode, to: span.node, span });
      }
    }
    const children = t.waiting.get(span.span_id);
    if (children) {
      t.waiting.delete(span.span_id);
      const from =
        span.node ?? (span.parent_span_id ? this.nearestJoined(t, span.parent_span_id) : null);
      for (const child of children) {
        if (from === undefined) {
          const list = t.waiting.get(span.parent_span_id!);
          if (list) list.push(child);
          else t.waiting.set(span.parent_span_id!, [child]);
        } else if (from && child.node && from !== child.node) {
          hops.push({ from, to: child.node, span: child });
        }
      }
    }
    return hops;
  }

  /** Node of the closest ancestor that was joined; `null` when the chain is known but unjoined, `undefined` when an ancestor is still missing. */
  private nearestJoined(t: TraceState, spanId: string): string | null | undefined {
    let cur: string | null = spanId;
    for (let i = 0; cur && i < 64; i++) {
      if (!t.nodeOf.has(cur)) return undefined;
      const node = t.nodeOf.get(cur);
      if (node) return node;
      cur = t.parentOf.get(cur) ?? null;
    }
    return null;
  }

  nodeOfSpan(traceId: string, spanId: string): string | null | undefined {
    return this.traces.get(traceId)?.nodeOf.get(spanId);
  }

  get size(): number {
    return this.traces.size;
  }

  private trace(id: string): TraceState {
    let t = this.traces.get(id);
    if (!t) {
      t = { nodeOf: new Map(), parentOf: new Map(), waiting: new Map() };
      this.traces.set(id, t);
      if (this.traces.size > this.maxTraces) {
        const oldest = this.traces.keys().next().value;
        if (oldest !== undefined) this.traces.delete(oldest);
      }
    }
    return t;
  }
}

export interface ReplayEvent {
  at: number;
  span: RuntimeSpan;
  from: string | null;
}

/** Relative schedule (ms from trace start) for replaying a trace on the graph, ordered by span start. */
export function scheduleReplay(spans: RuntimeSpan[]): ReplayEvent[] {
  if (spans.length === 0) return [];
  const t0 = Math.min(...spans.map((s) => s.start_unix_nanos));
  const byId = new Map(spans.map((s) => [s.span_id, s]));
  const nodeOfAncestor = (s: RuntimeSpan): string | null => {
    let p = s.parent_span_id ? byId.get(s.parent_span_id) : undefined;
    while (p) {
      if (p.node) return p.node;
      p = p.parent_span_id ? byId.get(p.parent_span_id) : undefined;
    }
    return null;
  };
  return spans
    .map((span) => ({ at: (span.start_unix_nanos - t0) / 1e6, span, from: nodeOfAncestor(span) }))
    .sort((a, b) => a.at - b.at);
}

export interface WaterfallRow {
  span: RuntimeSpan;
  depth: number;
  offsetMs: number;
  durationMs: number;
  /** Share of the duration covered by child spans (time waiting on children). */
  waitRatio: number;
}

export function waterfall(spans: RuntimeSpan[]): { rows: WaterfallRow[]; totalMs: number } {
  if (spans.length === 0) return { rows: [], totalMs: 0 };
  const t0 = Math.min(...spans.map((s) => s.start_unix_nanos));
  const t1 = Math.max(...spans.map((s) => s.end_unix_nanos));
  const byId = new Map(spans.map((s) => [s.span_id, s]));
  const children = new Map<string, RuntimeSpan[]>();
  for (const s of spans) {
    if (s.parent_span_id && byId.has(s.parent_span_id)) {
      const list = children.get(s.parent_span_id);
      if (list) list.push(s);
      else children.set(s.parent_span_id, [s]);
    }
  }
  const depthOf = (s: RuntimeSpan): number => {
    let d = 0;
    let p = s.parent_span_id ? byId.get(s.parent_span_id) : undefined;
    while (p) {
      d++;
      p = p.parent_span_id ? byId.get(p.parent_span_id) : undefined;
    }
    return d;
  };
  const rows = spans
    .slice()
    .sort((a, b) => a.start_unix_nanos - b.start_unix_nanos || depthOf(a) - depthOf(b))
    .map((span) => {
      const dur = Math.max(1, span.end_unix_nanos - span.start_unix_nanos);
      const kids = (children.get(span.span_id) ?? [])
        .map(
          (c) =>
            [
              Math.max(c.start_unix_nanos, span.start_unix_nanos),
              Math.min(c.end_unix_nanos, span.end_unix_nanos),
            ] as const,
        )
        .filter(([a, b]) => b > a)
        .sort((x, y) => x[0] - y[0]);
      let covered = 0;
      let cursor = -Infinity;
      for (const [a, b] of kids) {
        const start = Math.max(a, cursor);
        if (b > start) covered += b - start;
        cursor = Math.max(cursor, b);
      }
      return {
        span,
        depth: depthOf(span),
        offsetMs: (span.start_unix_nanos - t0) / 1e6,
        durationMs: dur / 1e6,
        waitRatio: Math.min(1, covered / dur),
      };
    });
  return { rows, totalMs: Math.max(1, (t1 - t0) / 1e6) };
}
