import { Priority, Status } from "../status";

export function badge(status: Status): string[] {
  switch (status) {
    case "active":
      return ["green"];
    case "paused":
      return ["grey"];
    default:
      return [];
  }
}

export function weight(p: Priority): number {
  switch (p) {
    case Priority.Low:
      return 1;
    case Priority.High:
      return 2;
    default:
      throw new Error(`unknown priority ${p}`);
  }
}

export function icon(kind: string): string {
  if (kind === Kind.Note || kind === Kind.Task) {
    return "doc";
  } else {
    return "";
  }
}

export function full(status: Status): string {
  if (status === "active" || status === "paused") {
    return "live";
  } else if (status === "done" || status === "archived") {
    return "closed";
  } else {
    return "?";
  }
}

export function twoLetters(code: string): number {
  switch (code) {
    case "ab":
      return 1;
    case "cd":
      return 2;
    default:
      return 0;
  }
}
