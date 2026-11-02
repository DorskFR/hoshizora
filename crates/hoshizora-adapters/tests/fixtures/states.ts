export enum Status {
  Active = "active",
  Paused = "paused",
  Done = "done",
}

export type Phase = "idle" | "running" | "failed";

export const MODES = ["fast", "slow"] as const;

export const KINDS = { Note: "note", Task: "task" } as const;

type Single = "only";

export function label(status: Status): string {
  switch (status) {
    case Status.Active:
      return "A";
    case Status.Paused:
      return "P";
    default:
      return [];
  }
}

export function phaseColor(phase: Phase) {
  if (phase === "idle" || phase === "running") {
    return "green";
  } else if (phase === "failed") {
    return "red";
  } else {
    throw new Error(`unknown phase ${phase}`);
  }
}

export function modeOk(mode: string) {
  if (["fast", "slow"].includes(mode)) {
    return true;
  }
  return false;
}

export function noop(x: Status) {
  switch (x) {
    case Status.Active:
      break;
    default:
  }
  switch (x.length) {
    case 1:
      return 1;
    default:
      assertNever(x);
  }
}
