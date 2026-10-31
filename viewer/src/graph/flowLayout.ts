import type { EdgeKind, FlowStep } from "../types";

export interface FlowBox {
  index: number;
  node: string;
  lane: string;
  row: number;
  col: number;
  depth: number;
}

export interface FlowArrow {
  from: number;
  to: number;
  kind: EdgeKind;
}

export interface FlowLayout {
  lanes: string[];
  boxes: FlowBox[];
  arrows: FlowArrow[];
  cols: number;
}

/**
 * Swimlane layout for `Subgraph.steps`: one column per step in DFS order, one row per lane
 * (first appearance order). Each step is linked to the most recent step one level shallower.
 */
export function layoutFlow(steps: FlowStep[]): FlowLayout {
  const lanes: string[] = [];
  const laneRow = new Map<string, number>();
  const boxes: FlowBox[] = [];
  const arrows: FlowArrow[] = [];
  const lastAtDepth: number[] = [];
  steps.forEach((step, index) => {
    let row = laneRow.get(step.lane);
    if (row === undefined) {
      row = lanes.length;
      lanes.push(step.lane);
      laneRow.set(step.lane, row);
    }
    boxes.push({ index, node: step.node, lane: step.lane, row, col: index, depth: step.depth });
    if (step.depth > 0 && step.via) {
      const parent = lastAtDepth[step.depth - 1];
      if (parent !== undefined) arrows.push({ from: parent, to: index, kind: step.via });
    }
    lastAtDepth[step.depth] = index;
    lastAtDepth.length = step.depth + 1;
  });
  return { lanes, boxes, arrows, cols: steps.length };
}
