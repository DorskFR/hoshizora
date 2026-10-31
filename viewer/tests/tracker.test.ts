import { describe, expect, it } from "vitest";
import { buildTrace, demoIds } from "../fixtures/demo";
import { SpanTracker, scheduleReplay, waterfall } from "../src/runtime/tracker";
import type { RuntimeSpan } from "../src/types";

function span(
  id: string,
  parent: string | null,
  node: string | null,
  start = 0,
  end = 10,
): RuntimeSpan {
  return {
    trace_id: "t",
    span_id: id,
    parent_span_id: parent,
    service: "svc",
    name: id,
    kind: "internal",
    start_unix_nanos: start * 1e6,
    end_unix_nanos: end * 1e6,
    error: false,
    attrs: {},
    node,
    join: node ? "code_function" : null,
  };
}

describe("SpanTracker", () => {
  it("emits a hop from the parent's node when spans arrive in order", () => {
    const t = new SpanTracker();
    expect(t.record(span("a", null, "fn:A"))).toEqual([]);
    const hops = t.record(span("b", "a", "fn:B"));
    expect(hops).toHaveLength(1);
    expect(hops[0]).toMatchObject({ from: "fn:A", to: "fn:B" });
  });

  it("resolves the hop later when the child is exported before its parent", () => {
    const t = new SpanTracker();
    expect(t.record(span("b", "a", "fn:B"))).toEqual([]);
    const hops = t.record(span("a", null, "fn:A"));
    expect(hops).toHaveLength(1);
    expect(hops[0]).toMatchObject({ from: "fn:A", to: "fn:B" });
  });

  it("skips unjoined intermediate spans and never hops onto the same node", () => {
    const t = new SpanTracker();
    t.record(span("a", null, "fn:A"));
    t.record(span("mid", "a", null));
    expect(t.record(span("c", "mid", "fn:C"))[0]).toMatchObject({ from: "fn:A", to: "fn:C" });
    expect(t.record(span("d", "c", "fn:C"))).toEqual([]);
  });

  it("evicts the oldest trace beyond the cap", () => {
    const t = new SpanTracker(2);
    for (const id of ["1", "2", "3"]) t.record({ ...span("s", null, "fn:X"), trace_id: id });
    expect(t.size).toBe(2);
    expect(t.nodeOfSpan("1", "s")).toBeUndefined();
    expect(t.nodeOfSpan("3", "s")).toBe("fn:X");
  });

  it("produces one hop per edge of the demo trace regardless of export order", () => {
    const trace = buildTrace("demo", 0);
    const reversed = [...trace.spans].reverse();
    const t = new SpanTracker();
    const hops = reversed.flatMap((s) => t.record(s));
    expect(hops.map((h) => `${h.from}>${h.to}`)).toContain(
      `${demoIds.create_order}>${demoIds.insert_order}`,
    );
    expect(hops).toHaveLength(trace.spans.length - 1);
  });
});

describe("scheduleReplay", () => {
  it("orders events by relative start and attaches the nearest joined ancestor node", () => {
    const trace = buildTrace("demo", 5_000_000_000);
    const events = scheduleReplay(trace.spans);
    expect(events[0]!.at).toBe(0);
    expect(events[0]!.span.node).toBe(demoIds.onSubmit);
    expect(events.every((e, i) => i === 0 || e.at >= events[i - 1]!.at)).toBe(true);
    const reserve = events.find((e) => e.span.node === demoIds.reserve)!;
    expect(reserve.from).toBe("route:POST /reserve");
  });
});

describe("waterfall", () => {
  it("computes offsets, depths and wait ratio from child coverage", () => {
    const rows = waterfall([
      span("root", null, "fn:R", 0, 100),
      span("c1", "root", "fn:C", 10, 40),
      span("c2", "root", "fn:C", 30, 60),
    ]);
    expect(rows.totalMs).toBe(100);
    const root = rows.rows.find((r) => r.span.span_id === "root")!;
    expect(root.depth).toBe(0);
    expect(root.waitRatio).toBeCloseTo(0.5);
    const c2 = rows.rows.find((r) => r.span.span_id === "c2")!;
    expect(c2.depth).toBe(1);
    expect(c2.offsetMs).toBe(30);
    expect(c2.durationMs).toBe(30);
    expect(c2.waitRatio).toBe(0);
  });
});
