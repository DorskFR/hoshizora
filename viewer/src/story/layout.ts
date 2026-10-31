import type { Story, StoryLane, StoryLink, StoryStep } from "../types";

export const LANE_W = 250;
export const BOX_W = 206;
export const BOX_H = 64;
export const COMPACT_H = 34;
export const HELPER_ROW = 18;
export const ROW_GAP = 26;
export const TOP = 24;
const GUTTER = 12;
const SLOT_W = 5;
const SLOTS = 3;
const LABEL_MAX = 380;

export interface LaneCol extends StoryLane {
  index: number;
  x: number;
  w: number;
  cx: number;
}

export interface StepBox {
  step: StoryStep;
  lane: LaneCol;
  row: number;
  x: number;
  y: number;
  w: number;
  h: number;
  /** Routes, tables and hosts: a single-line pill. */
  compact: boolean;
  expanded: boolean;
}

export interface Arrow {
  link: StoryLink;
  /** Polyline from the source to the target box edge. */
  points: [number, number][];
  label: { x: number; y: number; maxW: number; anchor: "start" | "middle" | "end" } | null;
  /** The target sits above the source: a reply or a write to a table already shown. */
  back: boolean;
}

export interface StoryLayout {
  lanes: LaneCol[];
  boxes: StepBox[];
  arrows: Arrow[];
  width: number;
  height: number;
}

const COMPACT_KINDS = new Set(["route", "table", "host", "topic", "dependency"]);

/**
 * Sequence-diagram layout: one column per lane in the order given (lanes only referenced by steps
 * are appended), one row per step in `seq` order. A cross-lane link leaves the side of its source,
 * runs down the gutter beside it and enters the target at the target's row; a link to a step above
 * leaves at the source's row and climbs the gutter beside the target. Inside a lane a link is a
 * short drop to the next row or a bracket on the right.
 */
export function layoutStory(story: Story, expanded: ReadonlySet<string> = new Set()): StoryLayout {
  const lanes: LaneCol[] = [];
  const laneById = new Map<string, LaneCol>();
  const addLane = (l: StoryLane) => {
    if (laneById.has(l.id)) return;
    const col: LaneCol = {
      ...l,
      index: lanes.length,
      x: lanes.length * LANE_W,
      w: LANE_W,
      cx: lanes.length * LANE_W + LANE_W / 2,
    };
    lanes.push(col);
    laneById.set(l.id, col);
  };
  for (const l of story.lanes) addLane(l);
  const steps = [...story.steps].sort((a, b) => a.seq - b.seq);
  for (const s of steps)
    if (!laneById.has(s.lane)) addLane({ id: s.lane, label: s.lane, kind: "service" });

  const boxes: StepBox[] = [];
  const boxByNode = new Map<string, StepBox>();
  let y = TOP;
  steps.forEach((step, row) => {
    const lane = laneById.get(step.lane)!;
    const compact =
      COMPACT_KINDS.has(step.kind) && step.folded.length === 0 && step.effects.length === 0;
    const open = expanded.has(step.node) && step.folded.length > 0;
    const h = compact ? COMPACT_H : BOX_H + (open ? step.folded.length * HELPER_ROW + 6 : 0);
    const w = compact
      ? Math.min(lane.w + 20, Math.max(BOX_W, textWidth(step.label, 12, true) + 52))
      : BOX_W;
    const box: StepBox = {
      step,
      lane,
      row,
      x: lane.cx - w / 2,
      y,
      w,
      h,
      compact,
      expanded: open,
    };
    boxes.push(box);
    if (!boxByNode.has(step.node)) boxByNode.set(step.node, box);
    y += h + ROW_GAP;
  });

  const incoming = new Map<StepBox, number>();
  const slots = new Map<StepBox, number>();
  const used = new Map<LaneCol, number>();
  const slotOf = (b: StepBox) => {
    let s = slots.get(b);
    if (s === undefined) {
      s = (used.get(b.lane) ?? 0) % SLOTS;
      used.set(b.lane, s + 1);
      slots.set(b, s);
    }
    return s * SLOT_W;
  };
  const arrows: Arrow[] = [];
  const links = [...story.links].sort((a, b) => a.seq - b.seq);
  for (const link of links) {
    const from = boxByNode.get(link.from);
    const to = boxByNode.get(link.to);
    if (!from || !to || from === to) continue;
    const back = to.row < from.row;
    const anchorBox = back ? from : to;
    const n = incoming.get(anchorBox) ?? 0;
    incoming.set(anchorBox, n + 1);
    const yMid = anchorBox.y + Math.min(anchorBox.h, BOX_H) / 2 + spread(n);
    arrows.push(route(link, from, to, back, yMid, slotOf(back ? to : from)));
  }

  return {
    lanes,
    boxes,
    arrows,
    width: Math.max(lanes.length, 1) * LANE_W,
    height: y - ROW_GAP + TOP,
  };
}

function spread(n: number): number {
  return n === 0 ? 0 : (n % 2 ? -1 : 1) * Math.ceil(n / 2) * 10;
}

/** `off` separates the vertical runs of different trunk owners in the same gutter. */
function route(
  link: StoryLink,
  from: StepBox,
  to: StepBox,
  back: boolean,
  yMid: number,
  off: number,
): Arrow {
  if (from.lane === to.lane) {
    const right = from.x + from.w;
    if (!back && to.row === from.row + 1) {
      const x = from.lane.cx;
      return {
        link,
        back,
        points: [
          [x, from.y + from.h],
          [x, to.y],
        ],
        label: link.label
          ? {
              x: x + 8,
              y: (from.y + from.h + to.y) / 2 + 4,
              maxW: from.lane.w / 2 - 12,
              anchor: "start",
            }
          : null,
      };
    }
    const bx = right + GUTTER + off;
    const y1 = from.y + Math.min(from.h, BOX_H) / 2;
    const y2 = to.y + Math.min(to.h, BOX_H) / 2;
    return {
      link,
      back,
      points: [
        [right, y1],
        [bx, y1],
        [bx, y2],
        [right, y2],
      ],
      label: link.label
        ? { x: bx + 4, y: (y1 + y2) / 2, maxW: from.lane.x + from.lane.w - bx - 6, anchor: "start" }
        : null,
    };
  }
  const rightward = to.lane.cx > from.lane.cx;
  if (!back) {
    const exit = rightward ? from.x + from.w : from.x;
    const gutter = exit + (rightward ? GUTTER + off : -GUTTER - off);
    const y0 = from.y + Math.min(from.h, BOX_H) / 2;
    const edge = rightward ? to.x : to.x + to.w;
    return {
      link,
      back,
      points: [
        [exit, y0],
        [gutter, y0],
        [gutter, yMid],
        [edge, yMid],
      ],
      label: link.label ? beside(edge, yMid, rightward ? "end" : "start") : null,
    };
  }
  const exit = rightward ? from.x + from.w : from.x;
  const enter = rightward ? to.x : to.x + to.w;
  const gutter = enter + (rightward ? -GUTTER - off : GUTTER + off);
  const y1 = to.y + Math.min(to.h, BOX_H) / 2;
  return {
    link,
    back,
    points: [
      [exit, yMid],
      [gutter, yMid],
      [gutter, y1],
      [enter, y1],
    ],
    label: link.label ? beside(exit, yMid, rightward ? "start" : "end") : null,
  };
}

/**
 * A cross-lane arrow runs along the row of a single box, so the rest of that row is free: the label
 * sits on the arrow next to that box and grows away from it.
 */
function beside(x: number, y: number, anchor: "start" | "end"): Arrow["label"] {
  const pad = 10;
  return anchor === "end"
    ? { x: x - pad, y: y - 6, maxW: Math.min(LABEL_MAX, x - pad - 4), anchor }
    : { x: x + pad, y: y - 6, maxW: LABEL_MAX, anchor };
}

/** Rough text width for 12px UI text, used to truncate labels without measuring. */
export function textWidth(s: string, px = 12, mono = false): number {
  return s.length * px * (mono ? 0.62 : 0.58);
}

export function fitText(s: string, maxW: number, px = 12): string {
  if (textWidth(s, px) <= maxW) return s;
  const n = Math.max(1, Math.floor(maxW / (px * 0.58)) - 1);
  return `${s.slice(0, n)}…`;
}
