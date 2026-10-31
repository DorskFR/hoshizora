import { describe, expect, it } from "vitest";
import { demoGraph, demoIds } from "../fixtures/demo";
import { syntheticGraph } from "../fixtures/synthetic";
import { buildView, containersToExpand, representative } from "../src/graph/lod";
import { GraphIndex } from "../src/graph/query";
import { EFFECT_KINDS } from "../src/types";

const index = new GraphIndex(demoGraph);
const allEffects = new Set(EFFECT_KINDS);
const base = { maxLinks: 10_000, effectFilter: allEffects, transitive: false };

describe("buildView", () => {
  it("collapses code into packages and keeps synthetic nodes", () => {
    const view = buildView(index, { ...base, level: "packages", expanded: new Set() });
    const ids = view.nodes.map((n) => n.id).sort();
    expect(ids.filter((id) => id.startsWith("mod:"))).toEqual([
      "mod:crates/api",
      "mod:webui",
      "mod:worker",
    ]);
    expect(ids).toContain("table:orders");
    expect(ids).toContain("route:POST /api/orders");
    expect(ids.some((id) => id.startsWith("fn:") || id.startsWith("file:"))).toBe(false);
  });

  it("aggregates edges between representatives with weights and drops self links", () => {
    const view = buildView(index, { ...base, level: "packages", expanded: new Set() });
    const api = view.nodes.find((n) => n.id === "mod:crates/api")!;
    expect(api.aggregate).toBe(true);
    expect(api.members).toContain(demoIds.create_order);
    const toWorker = view.links.find(
      (l) => l.source === "mod:crates/api" && l.target === "route:POST /reserve",
    );
    expect(toWorker?.kind).toBe("requests");
    const inner = view.links.filter((l) => l.source === l.target);
    expect(inner).toHaveLength(0);
    const webToApi = view.links.find(
      (l) => l.source === "mod:webui" && l.target === "route:POST /api/orders",
    );
    expect(webToApi?.weight).toBe(1);
  });

  it("expanding a package reveals its files; expanding a file reveals functions", () => {
    const expanded = new Set(["mod:worker"]);
    let view = buildView(index, { ...base, level: "packages", expanded });
    expect(view.nodes.map((n) => n.id)).toContain("file:worker/app/main.py");
    expect(view.nodes.map((n) => n.id)).not.toContain("mod:worker");
    expect(view.nodes.map((n) => n.id)).not.toContain(demoIds.reserve);
    expanded.add("file:worker/app/main.py");
    view = buildView(index, { ...base, level: "packages", expanded });
    expect(view.nodes.map((n) => n.id)).toContain(demoIds.reserve);
    expect(view.repOf.get(demoIds.check_inventory)).toBe("file:worker/app/inventory.py");
  });

  it("function level shows every function with dominant effect, dead flag and entrypoint", () => {
    const view = buildView(index, { ...base, level: "functions", expanded: new Set() });
    const insert = view.nodes.find((n) => n.id === demoIds.insert_order)!;
    expect(insert.effect).toBe("db_write");
    expect(view.nodes.find((n) => n.id === demoIds.legacy_format)!.dead).toBe(true);
    expect(view.nodes.find((n) => n.id === demoIds.onSubmit)!.entrypoint).toBe("ui_handler");
    expect(view.nodes.find((n) => n.id === demoIds.validate)!.pkg).toBe("mod:crates/api");
  });

  it("honours the effect filter and the transitive toggle", () => {
    const direct = buildView(index, { ...base, level: "functions", expanded: new Set() });
    expect(direct.nodes.find((n) => n.id === demoIds.create_order)!.effect).toBe("spawn");
    const transitive = buildView(index, {
      ...base,
      level: "functions",
      expanded: new Set(),
      transitive: true,
    });
    expect(transitive.nodes.find((n) => n.id === demoIds.create_order)!.effects).toContain(
      "db_write",
    );
    const noSpawn = new Set(EFFECT_KINDS.filter((k) => k !== "spawn"));
    const filtered = buildView(index, {
      ...base,
      level: "functions",
      expanded: new Set(),
      effectFilter: noSpawn,
    });
    expect(filtered.nodes.find((n) => n.id === demoIds.create_order)!.effect).toBeNull();
  });

  it("caps links keeping cross-process kinds first", () => {
    const view = buildView(index, {
      ...base,
      level: "functions",
      expanded: new Set(),
      maxLinks: 5,
    });
    expect(view.links).toHaveLength(5);
    expect(view.dropped).toBeGreaterThan(0);
    expect(view.links.every((l) => l.kind !== "calls")).toBe(true);
  });

  it("representative returns null for opened containers", () => {
    expect(representative(index, "mod:webui", "files", new Set())).toBeNull();
    expect(representative(index, "file:webui/src/api/client.ts", "files", new Set())).toBe(
      "file:webui/src/api/client.ts",
    );
    expect(representative(index, "table:orders", "packages", new Set())).toBe("table:orders");
  });

  it("containersToExpand lists what to open to reveal the given nodes", () => {
    expect(containersToExpand(index, [demoIds.reserve], "packages")).toEqual(
      new Set(["mod:worker", "file:worker/app/main.py"]),
    );
    expect(containersToExpand(index, [demoIds.reserve], "files")).toEqual(
      new Set(["file:worker/app/main.py"]),
    );
    expect(containersToExpand(index, [demoIds.reserve, "table:orders"], "functions").size).toBe(0);
  });
});

describe("synthetic graph", () => {
  it("builds the requested number of functions and aggregates them fast", () => {
    const g = syntheticGraph({ functions: 20_000 });
    const fns = g.nodes.filter((n) => n.kind === "function");
    expect(fns).toHaveLength(20_000);
    expect(g.edges.length).toBeGreaterThan(80_000);
    const idx = new GraphIndex(g);
    const t = performance.now();
    const view = buildView(idx, { ...base, level: "files", expanded: new Set(), maxLinks: 8000 });
    expect(performance.now() - t).toBeLessThan(1500);
    expect(view.nodes.filter((n) => n.kind === "file")).toHaveLength(480);
    expect(view.links.length).toBeLessThanOrEqual(8000);
  });
});
