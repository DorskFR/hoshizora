import { afterEach, describe, expect, it, vi } from "vitest";
import { cctuiSpawnStory } from "../fixtures/cctui";
import { HttpBackend, HttpProjects } from "../src/api";
import { MockProjects } from "../src/mock/backend";
import { dedent } from "../src/ui/panel";

function respond(body: unknown, init: { status?: number; type?: string } = {}) {
  const text = typeof body === "string" ? body : JSON.stringify(body);
  return new Response(init.status === 204 ? null : text, {
    status: init.status ?? 200,
    headers: { "content-type": init.type ?? "application/json" },
  });
}

afterEach(() => vi.unstubAllGlobals());

describe("HttpBackend", () => {
  it("sends every call through the project prefix", async () => {
    const urls: string[] = [];
    vi.stubGlobal("fetch", async (url: string) => {
      urls.push(url);
      return respond(url.includes("/story") ? cctuiSpawnStory : []);
    });
    const b = new HttpBackend("/api/projects/dorskfr-cctui");
    await b.search("spawn", 5);
    await b.node("fn:a.rs#f");
    await b.story("fn:a.rs#f");
    expect(urls).toEqual([
      "/api/projects/dorskfr-cctui/search?q=spawn&limit=5",
      "/api/projects/dorskfr-cctui/nodes/fn%3Aa.rs%23f",
      "/api/projects/dorskfr-cctui/story?from=fn%3Aa.rs%23f",
    ]);
  });

  it("reports a missing /api/story as null, whether 404 or the SPA's index.html", async () => {
    const b = new HttpBackend();
    vi.stubGlobal("fetch", async () => respond("<!doctype html>", { type: "text/html" }));
    expect(await b.story("x")).toBeNull();
    vi.stubGlobal("fetch", async () => respond("nope", { status: 404, type: "text/plain" }));
    expect(await b.story("x")).toBeNull();
    vi.stubGlobal("fetch", async () => respond("boom", { status: 500, type: "text/plain" }));
    await expect(b.story("x")).rejects.toThrow(/HTTP 500/);
  });
});

describe("HttpProjects", () => {
  it("returns null on servers without projects so the viewer opens the unprefixed API", async () => {
    vi.stubGlobal("fetch", async () => respond("<!doctype html>", { type: "text/html" }));
    expect(await new HttpProjects().list()).toBeNull();
  });

  it("posts a git URL and ref, and deletes with 204", async () => {
    const calls: { url: string; init?: RequestInit }[] = [];
    vi.stubGlobal("fetch", async (url: string, init?: RequestInit) => {
      calls.push({ url, init });
      return init?.method === "DELETE" ? respond(null, { status: 204 }) : respond({ id: "x" });
    });
    const api = new HttpProjects();
    await api.add({ url: "https://github.com/DorskFR/cctui", ref: "main" });
    await api.reindex("x");
    await api.remove("x");
    expect(calls.map((c) => `${c.init?.method ?? "GET"} ${c.url}`)).toEqual([
      "POST /api/projects",
      "POST /api/projects/x/reindex",
      "DELETE /api/projects/x",
    ]);
    expect(JSON.parse(String(calls[0]!.init!.body))).toEqual({
      url: "https://github.com/DorskFR/cctui",
      ref: "main",
    });
  });
});

describe("MockProjects", () => {
  it("walks an added repository through cloning and indexing to ready", async () => {
    vi.useFakeTimers();
    const api = new MockProjects(null);
    const p = await api.add({ url: "https://github.com/DorskFR/tsumikit", ref: null });
    expect(p).toMatchObject({ id: "dorskfr-tsumikit", status: "cloning", name: "tsumikit" });
    vi.advanceTimersByTime(2000);
    expect((await api.get(p.id)).status).toBe("indexing");
    vi.advanceTimersByTime(2000);
    expect((await api.get(p.id)).status).toBe("ready");
    expect((await api.add({ url: "https://github.com/DorskFR/tsumikit" })).id).toBe(p.id);
    vi.useRealTimers();
  });

  it("fails anything that is not a git URL", async () => {
    vi.useFakeTimers();
    const api = new MockProjects(null);
    const p = await api.add({ url: "not a url" });
    vi.advanceTimersByTime(2000);
    expect(await api.get(p.id)).toMatchObject({ status: "failed" });
    vi.useRealTimers();
  });

  it("serves the cctui spawn story", async () => {
    const story = await new MockProjects(null).backend("dorskfr-cctui").story(cctuiSpawnStory.root);
    expect(story).toBe(cctuiSpawnStory);
  });
});

describe("dedent", () => {
  it("strips shared indentation and keeps relative indentation", () => {
    expect(dedent("    a {\n      b\n\n    }")).toBe("a {\n  b\n\n}");
    expect(dedent("a\n  b")).toBe("a\n  b");
  });
});
