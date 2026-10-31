import { describe, expect, it } from "vitest";
import { cctuiGraph, cctuiIds, cctuiSpawnStory } from "../fixtures/cctui";
import { demoGraph, demoIds } from "../fixtures/demo";
import { flowFrom, GraphIndex } from "../src/graph/query";
import { storyFromFlow } from "../src/story/build";
import { BOX_H, LANE_W, layoutStory, type StepBox } from "../src/story/layout";
import type { Story } from "../src/types";

const I = cctuiIds;

function boxOf(boxes: StepBox[], node: string): StepBox {
  const b = boxes.find((x) => x.step.node === node);
  if (!b) throw new Error(`no box for ${node}`);
  return b;
}

describe("layoutStory", () => {
  const layout = layoutStory(cctuiSpawnStory);

  it("keeps lanes in the given order as equal-width columns", () => {
    expect(layout.lanes.map((l) => l.id)).toEqual([
      "webui",
      "server",
      "daemon",
      "postgres",
      "external",
    ]);
    layout.lanes.forEach((l, i) => {
      expect(l.x).toBe(i * LANE_W);
      expect(l.cx).toBe(i * LANE_W + LANE_W / 2);
    });
    expect(layout.width).toBe(5 * LANE_W);
  });

  it("appends lanes that only steps reference", () => {
    const story: Story = {
      ...cctuiSpawnStory,
      lanes: cctuiSpawnStory.lanes.filter((l) => l.id !== "external"),
    };
    const l = layoutStory(story);
    expect(l.lanes.at(-1)).toMatchObject({ id: "external", kind: "service", index: 4 });
  });

  it("places one step per row in seq order, centred in its lane", () => {
    const shuffled: Story = { ...cctuiSpawnStory, steps: [...cctuiSpawnStory.steps].reverse() };
    const l = layoutStory(shuffled);
    expect(l.boxes.map((b) => b.step.seq)).toEqual(cctuiSpawnStory.steps.map((s) => s.seq));
    for (let i = 1; i < l.boxes.length; i++) {
      const prev = l.boxes[i - 1]!;
      expect(l.boxes[i]!.y).toBeGreaterThanOrEqual(prev.y + prev.h);
    }
    for (const b of l.boxes) expect(b.x + b.w / 2).toBe(b.lane.cx);
    expect(boxOf(l.boxes, I.handle_spawn).lane.id).toBe("daemon");
    expect(boxOf(l.boxes, I.sessions).lane.id).toBe("postgres");
  });

  it("draws routes, tables and hosts as compact pills", () => {
    expect(boxOf(layout.boxes, I.routeSpawn).compact).toBe(true);
    expect(boxOf(layout.boxes, I.sessions).compact).toBe(true);
    expect(boxOf(layout.boxes, I.spawn_session).compact).toBe(false);
  });

  it("grows a step when its folded helpers are expanded", () => {
    const closed = boxOf(layout.boxes, I.spawn_session);
    const open = boxOf(
      layoutStory(cctuiSpawnStory, new Set([I.spawn_session])).boxes,
      I.spawn_session,
    );
    expect(closed.h).toBe(BOX_H);
    expect(open.expanded).toBe(true);
    expect(open.h).toBeGreaterThan(closed.h);
  });

  it("routes a forward cross-lane link from the source side into the target at the target row", () => {
    const a = layout.arrows.find(
      (x) => x.link.from === I.insert_session && x.link.to === I.sessions,
    )!;
    const from = boxOf(layout.boxes, I.insert_session);
    const to = boxOf(layout.boxes, I.sessions);
    expect(a.back).toBe(false);
    const [start, , , end] = a.points;
    expect(start![0]).toBe(from.x + from.w);
    expect(end![0]).toBe(to.x);
    expect(end![1]).toBeGreaterThan(to.y);
    expect(end![1]).toBeLessThan(to.y + to.h);
    expect(a.label).toMatchObject({ anchor: "end" });
    expect(a.label!.x).toBeLessThan(to.x);
  });

  it("routes a link to a step above from the source row up into the target", () => {
    const a = layout.arrows.find(
      (x) => x.link.from === I.ingest_event && x.link.to === I.sessions,
    )!;
    const from = boxOf(layout.boxes, I.ingest_event);
    const to = boxOf(layout.boxes, I.sessions);
    expect(a.back).toBe(true);
    const first = a.points[0]!;
    const last = a.points.at(-1)!;
    expect(first[0]).toBe(from.x + from.w);
    expect(first[1]).toBeGreaterThan(from.y);
    expect(first[1]).toBeLessThan(from.y + from.h);
    expect(last[0]).toBe(to.x);
    expect(a.points[2]![1]).toBeLessThan(from.y);
  });

  it("drops a link straight down between adjacent steps of the same lane", () => {
    const a = layout.arrows.find((x) => x.link.from === I.submit && x.link.to === I.spawnSession)!;
    const from = boxOf(layout.boxes, I.submit);
    const to = boxOf(layout.boxes, I.spawnSession);
    expect(a.points).toEqual([
      [from.lane.cx, from.y + from.h],
      [from.lane.cx, to.y],
    ]);
  });

  it("brackets a same-lane link that skips rows on the right of the boxes", () => {
    const a = layout.arrows.find(
      (x) => x.link.from === I.spawn_session && x.link.to === I.dispatch_spawn,
    )!;
    const from = boxOf(layout.boxes, I.spawn_session);
    expect(a.points).toHaveLength(4);
    expect(a.points[1]![0]).toBeGreaterThan(from.x + from.w);
    expect(a.points[1]![0]).toBe(a.points[2]![0]);
    expect(a.points[3]![0]).toBe(from.x + from.w);
  });

  it("keeps every vertical run outside the boxes of the rows it crosses", () => {
    for (const a of layout.arrows) {
      for (let i = 1; i < a.points.length; i++) {
        const [x1, y1] = a.points[i - 1]!;
        const [x2, y2] = a.points[i]!;
        if (x1 !== x2 || Math.abs(y2 - y1) < 1) continue;
        const lo = Math.min(y1, y2);
        const hi = Math.max(y1, y2);
        for (const b of layout.boxes) {
          const crossesRow = b.y > lo && b.y + b.h < hi;
          if (crossesRow)
            expect(
              x1 > b.x && x1 < b.x + b.w,
              `${a.link.from} → ${a.link.to} crosses ${b.step.label}`,
            ).toBe(false);
        }
      }
    }
  });

  it("skips links whose ends are not steps", () => {
    const story: Story = {
      ...cctuiSpawnStory,
      links: [...cctuiSpawnStory.links, { ...cctuiSpawnStory.links[0]!, to: "fn:nowhere" }],
    };
    expect(layoutStory(story).arrows).toHaveLength(layout.arrows.length);
  });
});

describe("storyFromFlow", () => {
  const index = new GraphIndex(cctuiGraph);
  const story = storyFromFlow(index, flowFrom(index, I.submit));
  const nodes = story.steps.map((s) => s.node);

  it("keeps I/O steps, folds pure helpers and leaves tests out", () => {
    expect(nodes[0]).toBe(I.submit);
    expect(nodes).toContain(I.spawnSession);
    expect(nodes).toContain(I.spawn_session);
    expect(nodes).toContain(I.insert_session);
    expect(nodes).toContain(I.sessions);
    expect(nodes).not.toContain(I.validateSpawn);
    expect(nodes).not.toContain(I.test_row);
    expect(story.steps[0]!.folded).toEqual(
      expect.arrayContaining([I.validateSpawn, I.buildSpawnBody]),
    );
    expect(story.steps.map((s) => s.seq)).toEqual(story.steps.map((_, i) => i));
  });

  it("turns a route into the label of the link from caller to handler", () => {
    expect(nodes).not.toContain(I.routeSpawn);
    const link = story.links.find((l) => l.to === I.spawn_session)!;
    expect(link.from).toBe(I.spawnSession);
    expect(link.label).toBe("POST /api/v1/sessions/spawn");
  });

  it("orders lanes UI first, then services and workers, then data", () => {
    const kinds = story.lanes.map((l) => l.kind);
    expect(kinds[0]).toBe("ui");
    expect(kinds.indexOf("database")).toBeGreaterThan(kinds.indexOf("service"));
    expect(kinds.indexOf("external")).toBe(kinds.length - 1);
    const daemon = story.steps.find((s) => s.node === I.handle_spawn)!;
    expect(story.lanes.find((l) => l.id === daemon.lane)!.kind).toBe("worker");
  });

  it("marks spawned work as detached", () => {
    const spawn = story.links.find((l) => l.to === I.watch_spawn_timeout)!;
    expect(spawn.detached).toBe(true);
    expect(spawn.kind).toBe("spawns");
  });

  it("caps the number of steps", () => {
    const capped = storyFromFlow(index, flowFrom(index, I.submit), 4);
    expect(capped.steps).toHaveLength(4);
    expect(capped.truncated).toBe(true);
  });

  it("works on another codebase", () => {
    const demo = new GraphIndex(demoGraph);
    const s = storyFromFlow(demo, flowFrom(demo, demoIds.onSubmit));
    expect(s.steps.map((x) => x.node)).toContain("table:inventory");
    expect(layoutStory(s).boxes).toHaveLength(s.steps.length);
  });
});
