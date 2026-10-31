import { describe, expect, it } from "vitest";
import { cctuiGraph, cctuiIds } from "../fixtures/cctui";
import { isTestNode } from "../src/graph/filters";
import { buildView } from "../src/graph/lod";
import { GraphIndex } from "../src/graph/query";
import { EFFECT_KINDS } from "../src/types";
import { groupEntrypoints } from "../src/ui/entrypoints";

const index = new GraphIndex(cctuiGraph);
const base = {
  maxLinks: 10_000,
  effectFilter: new Set(EFFECT_KINDS),
  transitive: false,
  expanded: new Set<string>(),
};
const node = (id: string) => index.node(id)!;

describe("isTestNode", () => {
  it("recognises tests by entrypoint, path and module", () => {
    expect(isTestNode(node(cctuiIds.test_row))).toBe(true);
    expect(isTestNode(node("fn:webui/journeys/spawn-session.journey.ts#spawnSessionJourney"))).toBe(
      true,
    );
    const n = { entrypoint: null, file: "crates/a/src/lib.rs", qualified_name: "a::tests::helper" };
    expect(isTestNode(n)).toBe(true);
    expect(isTestNode({ ...n, file: "webui/src/a.test.ts", qualified_name: "a" })).toBe(true);
    expect(isTestNode({ ...n, file: "app/test_orders.py", qualified_name: "test_orders.x" })).toBe(
      true,
    );
  });

  it("leaves product code alone", () => {
    expect(isTestNode(node(cctuiIds.spawn_session))).toBe(false);
    expect(isTestNode(node(cctuiIds.submit))).toBe(false);
    expect(
      isTestNode({ entrypoint: null, file: "src/attest.rs", qualified_name: "x::attest" }),
    ).toBe(false);
  });
});

describe("calmer overview", () => {
  it("folds routes into the package of their handler at package level", () => {
    const view = buildView(index, { ...base, level: "packages", foldRoutes: true });
    const ids = view.nodes.map((n) => n.id);
    expect(ids.some((id) => id.startsWith("route:"))).toBe(false);
    expect(view.repOf.get(cctuiIds.routeSpawn)).toBe("mod:crates/cctui-server-workspace");
    const request = view.links.find(
      (l) => l.source === "mod:webui" && l.target === "mod:crates/cctui-server-workspace",
    );
    expect(request?.kind).toBe("requests");
  });

  it("shows a route again once its handler is visible", () => {
    const view = buildView(index, { ...base, level: "functions", foldRoutes: true });
    expect(view.nodes.map((n) => n.id)).toContain(cctuiIds.routeSpawn);
  });

  it("hides tests when asked", () => {
    const hidden = buildView(index, { ...base, level: "functions", hideTests: true });
    const shown = buildView(index, { ...base, level: "functions", hideTests: false });
    expect(hidden.nodes.map((n) => n.id)).not.toContain(cctuiIds.test_row);
    expect(shown.nodes.map((n) => n.id)).toContain(cctuiIds.test_row);
    expect(hidden.links.some((l) => l.source === cctuiIds.test_row)).toBe(false);
  });
});

describe("groupEntrypoints", () => {
  it("groups by what starts the flow and hides tests and exports by default", () => {
    const groups = groupEntrypoints(index, { tests: false, exports: false });
    expect(groups.map((g) => g.group.key)).toEqual(["ui", "http", "cli", "job"]);
    const all = groups.flatMap((g) => g.rows.map((r) => r.node.id));
    expect(all).not.toContain(cctuiIds.test_row);
    expect(groups[0]!.rows.map((r) => r.node.id)).toContain(cctuiIds.submit);
  });

  it("titles HTTP handlers by their route", () => {
    const http = groupEntrypoints(index, { tests: false, exports: false }).find(
      (g) => g.group.key === "http",
    )!;
    const spawn = http.rows.find((r) => r.node.id === cctuiIds.spawn_session)!;
    expect(spawn.title).toBe("POST /api/v1/sessions/spawn");
    expect(spawn.search).toContain("spawn_session");
  });

  it("lists tests when included", () => {
    const groups = groupEntrypoints(index, { tests: true, exports: false });
    expect(groups.at(-1)!.group.key).toBe("test");
    expect(groups.at(-1)!.rows.map((r) => r.node.id)).toContain(cctuiIds.test_row);
  });
});
