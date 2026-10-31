import { describe, expect, it } from "vitest";
import { demoGraph, demoIds } from "../fixtures/demo";
import { layoutFlow } from "../src/graph/flowLayout";
import { flowFrom, GraphIndex, impactOf } from "../src/graph/query";

const index = new GraphIndex(demoGraph);

describe("flowFrom", () => {
  it("follows the cross-process chain from the UI handler to the inventory table", () => {
    const flow = flowFrom(index, demoIds.onSubmit);
    expect(flow.root).toBe(demoIds.onSubmit);
    expect(flow.nodes).toContain("route:POST /api/orders");
    expect(flow.nodes).toContain(demoIds.create_order);
    expect(flow.nodes).toContain(demoIds.reserve);
    expect(flow.nodes).toContain("table:inventory");
    expect(flow.nodes).not.toContain(demoIds.legacy_format);
    expect(flow.steps[0]).toMatchObject({
      node: demoIds.onSubmit,
      depth: 0,
      via: null,
      lane: "webui",
    });
    const reserve = flow.steps.find((s) => s.node === demoIds.reserve)!;
    expect(reserve.lane).toBe("worker");
    expect(reserve.via).toBe("handles");
  });
});

describe("impactOf", () => {
  it("lists affected entrypoints with shortest paths", () => {
    const impact = impactOf(index, demoIds.check_inventory);
    const kinds = impact.entrypoints.map((e) => e.kind).sort();
    expect(kinds).toEqual(["http_handler", "http_handler", "job", "main", "test", "ui_handler"]);
    const ui = impact.entrypoints.find((e) => e.kind === "ui_handler")!;
    expect(ui.path[0]).toBe(demoIds.onSubmit);
    expect(ui.path.at(-1)).toBe(demoIds.check_inventory);
    expect(ui.path).toContain("route:POST /reserve");
  });
});

describe("layoutFlow", () => {
  it("assigns lanes in first-appearance order and arrows to the DFS parent", () => {
    const flow = flowFrom(index, demoIds.onSubmit);
    const layout = layoutFlow(flow.steps);
    expect(layout.lanes[0]).toBe("webui");
    expect(layout.lanes).toContain("network");
    expect(layout.lanes).toContain("database");
    expect(layout.cols).toBe(flow.steps.length);
    const reserveBox = layout.boxes.find((b) => b.node === demoIds.reserve)!;
    const arrow = layout.arrows.find((a) => a.to === reserveBox.index)!;
    expect(layout.boxes[arrow.from]!.node).toBe("route:POST /reserve");
    expect(arrow.kind).toBe("handles");
    // the second sibling of create_order links back to create_order, not to the previous leaf
    const insertBox = layout.boxes.find((b) => b.node === demoIds.insert_order)!;
    const insertArrow = layout.arrows.find((a) => a.to === insertBox.index)!;
    expect(layout.boxes[insertArrow.from]!.node).toBe(demoIds.create_order);
  });

  it("handles an empty flow", () => {
    expect(layoutFlow([])).toEqual({ lanes: [], boxes: [], arrows: [], cols: 0 });
  });
});
