import type { EdgeKind, EffectKind, NodeKind } from "../types";

export const EFFECT_COLORS: Record<EffectKind, string> = {
  network: "#4cc9f0",
  db_read: "#80ed99",
  db_write: "#f9c74f",
  db_delete: "#f8961e",
  fs_read: "#90be6d",
  fs_write: "#f3722c",
  process: "#f94144",
  spawn: "#c77dff",
  join: "#9d4edd",
  lock: "#ff6b6b",
  publish: "#ff9e00",
  subscribe: "#ffb703",
  sleep: "#8d99ae",
  blocking: "#e5383b",
};

export const EFFECT_ICONS: Record<EffectKind, string> = {
  network: "⇄",
  db_read: "⤓",
  db_write: "⤒",
  db_delete: "⌫",
  fs_read: "▤",
  fs_write: "▥",
  process: "⚙",
  spawn: "⑂",
  join: "⑃",
  lock: "🔒",
  publish: "↗",
  subscribe: "↙",
  sleep: "◷",
  blocking: "⛔",
};

export const SYNTHETIC_COLORS: Partial<Record<NodeKind, string>> = {
  route: "#48bfe3",
  table: "#f9c74f",
  host: "#5390d9",
  topic: "#ff9e00",
  dependency: "#8d99ae",
};

export const EDGE_COLORS: Record<EdgeKind, string> = {
  calls: "#9aa5c4",
  spawns: "#d8b4fe",
  requests: "#4cc9f0",
  handles: "#48bfe3",
  reads: "#80ed99",
  writes: "#f9c74f",
  publishes: "#ff9e00",
  subscribes: "#ffb703",
  uses: "#6c757d",
  implements: "#adb5bd",
  imports: "#6c757d",
  contains: "#495057",
};

export const NEUTRAL = "#c9d1e9";
export const DEAD = "#4a5063";
export const IMPACT = "#ff8c42";
export const FLOW = "#ffd166";

/** Distinct hue per package, golden-angle spaced. */
export function packageHue(index: number): number {
  return (index * 137.508) % 360;
}

export function packageColor(index: number, light = 62, sat = 70): string {
  if (index < 0) return NEUTRAL;
  return `hsl(${packageHue(index).toFixed(1)}, ${sat}%, ${light}%)`;
}

export function hslToHex(h: number, s: number, l: number): number {
  s /= 100;
  l /= 100;
  const k = (n: number) => (n + h / 30) % 12;
  const a = s * Math.min(l, 1 - l);
  const f = (n: number) => l - a * Math.max(-1, Math.min(k(n) - 3, Math.min(9 - k(n), 1)));
  return (Math.round(f(0) * 255) << 16) | (Math.round(f(8) * 255) << 8) | Math.round(f(4) * 255);
}

export function withAlpha(hex: string, alpha: number): string {
  const n = Number.parseInt(hex.slice(1), 16);
  const r = (n >> 16) & 255;
  const g = (n >> 8) & 255;
  const b = n & 255;
  return `rgba(${r},${g},${b},${alpha})`;
}
