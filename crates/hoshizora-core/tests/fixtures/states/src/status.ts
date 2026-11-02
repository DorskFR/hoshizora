export type Status = "active" | "paused" | "done" | "archived";

export enum Priority {
  Low = "low",
  High = "high",
}

export const Kind = { Note: "note", Task: "task", Event: "event" } as const;
