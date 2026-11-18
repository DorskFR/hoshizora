export type Filter = "pending" | "approved" | "rejected" | "all";
export type Tone = "ok" | "warn" | "error" | "pending" | "paused";

export function statusFilter(f: Filter) {
  switch (f) {
    case "pending":
      return "status = 'pending'";
    case "approved":
      return "status = 'approved'";
    case "rejected":
      return "status = 'rejected'";
    default:
      return undefined;
  }
}

export function toneOf(t: Tone) {
  switch (t) {
    case "ok":
      return "green";
    case "warn":
      return "amber";
    case "error":
      return "red";
    default:
      return "neutral";
  }
}

export function badge(t: Tone) {
  switch (t) {
    case "ok":
      return "ok";
    case "warn":
      return "warn";
    case "error":
      return "error";
    default:
      return null;
  }
}
