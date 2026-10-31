import { describe, expect, it } from "vitest";
import { apiBase, formatRoute, HOME, parseRoute, type Route } from "../src/router";

const NODE = "fn:crates/cctui-server-workspace/src/routes/spawn.rs#spawn_session";

describe("project routes", () => {
  it("parses home", () => {
    expect(parseRoute("")).toEqual(HOME);
    expect(parseRoute("#")).toEqual(HOME);
    expect(parseRoute("#/")).toEqual(HOME);
  });

  it("parses a project map, a selected node and a story", () => {
    expect(parseRoute("#/p/dorskfr-cctui")).toEqual({
      project: "dorskfr-cctui",
      view: "map",
      node: null,
    });
    expect(parseRoute(`#/p/dorskfr-cctui/node/${encodeURIComponent(NODE)}`)).toEqual({
      project: "dorskfr-cctui",
      view: "map",
      node: NODE,
    });
    expect(parseRoute(`#/p/dorskfr-cctui/story/${encodeURIComponent(NODE)}`)).toEqual({
      project: "dorskfr-cctui",
      view: "story",
      node: NODE,
    });
  });

  it("parses unprefixed routes for single-project servers", () => {
    expect(parseRoute("#/map")).toEqual({ project: null, view: "map", node: null });
    expect(parseRoute(`#/story/${encodeURIComponent(NODE)}`)).toEqual({
      project: null,
      view: "story",
      node: NODE,
    });
  });

  it("tolerates unencoded slashes and hashes inside node ids", () => {
    expect(parseRoute("#/p/x/story/fn:a/b.rs#f").node).toBe("fn:a/b.rs#f");
  });

  it("falls back to the map for a story without a node", () => {
    expect(parseRoute("#/p/x/story")).toEqual({ project: "x", view: "map", node: null });
  });

  it("round-trips every route", () => {
    const routes: Route[] = [
      HOME,
      { project: "dorskfr-cctui", view: "map", node: null },
      { project: "dorskfr-cctui", view: "map", node: NODE },
      { project: "dorskfr-cctui", view: "story", node: NODE },
      { project: "a b/c", view: "story", node: "route:POST /api/v1/sessions/{}" },
      { project: null, view: "map", node: null },
      { project: null, view: "story", node: NODE },
    ];
    for (const r of routes) expect(parseRoute(formatRoute(r))).toEqual(r);
  });

  it("keeps ids opaque in the URL", () => {
    expect(formatRoute({ project: "p", view: "story", node: NODE })).toBe(
      `#/p/p/story/${encodeURIComponent(NODE)}`,
    );
  });

  it("prefixes API calls with the project", () => {
    expect(apiBase("dorskfr-cctui")).toBe("/api/projects/dorskfr-cctui");
    expect(apiBase("a b")).toBe("/api/projects/a%20b");
    expect(apiBase(null)).toBe("/api");
  });
});
