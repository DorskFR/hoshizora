import { h } from "./dom";

const KEY = "hoshizora.panelWidth";
const MIN = 300;
const NARROW = 420;
const WIDE = 0.6;

export function clampPanelWidth(px: number, viewport: number): number {
  return Math.round(Math.min(Math.max(px, MIN), Math.max(MIN, viewport * 0.8)));
}

function stored(): number {
  const v = Number(localStorage.getItem(KEY));
  return Number.isFinite(v) && v > 0 ? v : NARROW;
}

/**
 * Drag handle on the left edge of the side panel. Sets `--panel-w` on `root`; the width persists in
 * localStorage. Double-click toggles between narrow and 60% of the window for reading code.
 */
export function panelResizer(root: HTMLElement): HTMLElement {
  const set = (px: number) => {
    const w = clampPanelWidth(px, window.innerWidth);
    root.style.setProperty("--panel-w", `${w}px`);
    return w;
  };
  set(stored());
  const handle = h("div", {
    class: "panel-resizer",
    role: "separator",
    "aria-label": "Resize side panel (double-click to widen)",
    title: "Drag to resize · double-click to widen for code",
    tabIndex: 0,
  });
  handle.setAttribute("aria-orientation", "vertical");
  let dragging = false;
  handle.addEventListener("pointerdown", (ev) => {
    dragging = true;
    handle.setPointerCapture(ev.pointerId);
    document.body.classList.add("resizing");
  });
  handle.addEventListener("pointermove", (ev) => {
    if (!dragging) return;
    set(window.innerWidth - ev.clientX - 10);
  });
  handle.addEventListener("pointerup", (ev) => {
    if (!dragging) return;
    dragging = false;
    handle.releasePointerCapture(ev.pointerId);
    document.body.classList.remove("resizing");
    localStorage.setItem(KEY, String(set(window.innerWidth - ev.clientX - 10)));
  });
  handle.addEventListener("dblclick", () => {
    const cur = stored();
    const wide = Math.round(window.innerWidth * WIDE);
    localStorage.setItem(KEY, String(set(cur >= wide - 10 ? NARROW : wide)));
  });
  handle.addEventListener("keydown", (ev) => {
    if (ev.key !== "ArrowLeft" && ev.key !== "ArrowRight") return;
    ev.preventDefault();
    const next = set(stored() + (ev.key === "ArrowLeft" ? 40 : -40));
    localStorage.setItem(KEY, String(next));
  });
  window.addEventListener("resize", () => set(stored()));
  return handle;
}
