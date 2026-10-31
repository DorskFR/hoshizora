import { EDGE_COLORS, EFFECT_COLORS } from "../graph/colors";
import type { GraphIndex } from "../graph/query";
import {
  type Arrow,
  BOX_H,
  fitText,
  HELPER_ROW,
  layoutStory,
  type StepBox,
  type StoryLayout,
  textWidth,
} from "../story/layout";
import type { Effect, EffectKind, LaneKind, Story } from "../types";
import { clear, h, shortId } from "./dom";

const SVG = "http://www.w3.org/2000/svg";
const HEADER_H = 48;

export const LANE_COLORS: Record<LaneKind, string> = {
  ui: "#7aa2ff",
  service: "#48bfe3",
  worker: "#c77dff",
  queue: "#ff9e00",
  database: "#f9c74f",
  external: "#8d99ae",
};

const LANE_HINT: Record<LaneKind, string> = {
  ui: "browser / UI",
  service: "service",
  worker: "worker / daemon",
  queue: "queue",
  database: "database",
  external: "external",
};

const EFFECT_WORDS: Record<EffectKind, string> = {
  network: "calls",
  db_read: "reads",
  db_write: "writes",
  fs_read: "reads file",
  fs_write: "writes file",
  process: "runs",
  spawn: "spawns",
  join: "joins",
  lock: "locks",
  publish: "publishes",
  subscribe: "subscribes",
  sleep: "sleeps",
  blocking: "blocks",
};

function svg<K extends keyof SVGElementTagNameMap>(
  tag: K,
  attrs: Record<string, string | number>,
  ...children: (Node | string)[]
): SVGElementTagNameMap[K] {
  const el = document.createElementNS(SVG, tag);
  for (const [k, v] of Object.entries(attrs)) el.setAttribute(k, String(v));
  for (const c of children) el.append(c);
  return el;
}

export function effectChip(e: Pick<Effect, "kind" | "target">): string {
  const target = e.target ? ` ${e.target}` : "";
  return `${EFFECT_WORDS[e.kind]}${target}`;
}

export interface StoryMeta {
  /** Built in the browser from `/api/flow` because the server has no `/api/story`. */
  approximate: boolean;
}

export interface StoryCallbacks {
  select(id: string): void;
  close(): void;
  showOnMap(story: Story): void;
}

export class StoryView {
  readonly el: HTMLElement;
  private readonly titleEl: HTMLElement;
  private readonly subEl: HTMLElement;
  private readonly noteEl: HTMLElement;
  private readonly stage: HTMLElement;
  private readonly lanesEl: HTMLElement;
  private readonly svgEl: SVGSVGElement;
  private readonly viewport: SVGGElement;
  private story: Story | null = null;
  private layout: StoryLayout | null = null;
  private expanded = new Set<string>();
  private selected: string | null = null;
  private k = 1;
  private tx = 0;
  private ty = HEADER_H;
  private drag: { x: number; y: number; tx: number; ty: number; moved: boolean } | null = null;

  constructor(
    private readonly index: GraphIndex,
    private readonly cb: StoryCallbacks,
  ) {
    this.titleEl = h("h2", { class: "story-name" });
    this.subEl = h("span", { class: "muted small" });
    this.noteEl = h("div", { class: "story-note", hidden: true });
    this.lanesEl = h("div", { class: "story-lanes" });
    this.svgEl = svg("svg", { class: "story-svg", role: "img", "aria-label": "Story diagram" });
    this.viewport = svg("g", {});
    this.svgEl.append(defs(), this.viewport);
    this.stage = h("div", { class: "story-stage" }, this.svgEl, this.lanesEl);
    const zoomBtn = (label: string, aria: string, fn: () => void) =>
      h("button", { class: "ghost small", "aria-label": aria, title: aria, onclick: fn }, label);
    this.el = h(
      "section",
      { class: "story", hidden: true, "aria-label": "Story" },
      h(
        "header",
        { class: "story-head" },
        h(
          "div",
          { class: "story-title" },
          h("span", { class: "eyebrow" }, "Story · what happens when this runs"),
          this.titleEl,
          this.subEl,
        ),
        h("span", { class: "spacer" }),
        h(
          "div",
          { class: "story-key small muted" },
          h("span", null, h("i", { class: "key-line" }), "awaited"),
          h("span", null, h("i", { class: "key-line dashed" }), "fire-and-forget"),
          h("span", null, h("i", { class: "key-line faint" }), "guessed by name"),
        ),
        h(
          "div",
          { class: "story-controls" },
          zoomBtn("−", "Zoom out", () => this.zoomBy(1 / 1.25)),
          zoomBtn("+", "Zoom in", () => this.zoomBy(1.25)),
          zoomBtn("Fit", "Fit to screen", () => this.fit("all")),
          zoomBtn("Show on map", "Highlight these steps in the 3D map", () => {
            if (this.story) this.cb.showOnMap(this.story);
          }),
          h(
            "button",
            { class: "ghost small", "aria-label": "Close story", onclick: () => this.cb.close() },
            "✕ Close",
          ),
        ),
      ),
      this.noteEl,
      this.stage,
    );
    this.bindPanZoom();
    new ResizeObserver(() => this.apply()).observe(this.stage);
  }

  get open(): boolean {
    return !this.el.hidden;
  }

  get root(): string | null {
    return this.story?.root ?? null;
  }

  showLoading(name: string): void {
    this.el.hidden = false;
    this.titleEl.textContent = name;
    this.subEl.textContent = "loading…";
    this.noteEl.hidden = true;
    clear(this.viewport);
    clear(this.lanesEl);
  }

  showError(name: string, message: string): void {
    this.showLoading(name);
    this.subEl.textContent = "";
    this.noteEl.hidden = false;
    this.noteEl.className = "story-note error";
    this.noteEl.textContent = message;
  }

  show(story: Story, meta: StoryMeta): void {
    this.el.hidden = false;
    this.story = story;
    this.expanded.clear();
    const root = story.steps.find((s) => s.node === story.root);
    this.titleEl.textContent =
      root?.label ?? this.index.node(story.root)?.name ?? shortId(story.root);
    const helpers = story.steps.reduce((n, s) => n + s.folded.length, 0);
    this.subEl.textContent = `${story.steps.length} steps across ${story.lanes.length} lanes${helpers ? ` · ${helpers} helpers folded` : ""}${story.truncated ? " · truncated" : ""} · click a step to read its code`;
    const notes: string[] = [];
    if (meta.approximate)
      notes.push(
        "Approximate: this server has no /api/story yet, so the story was condensed in the browser from the raw call flow.",
      );
    if (story.truncated) notes.push("Only the first steps are shown; the flow continues further.");
    if (story.steps.length === 1)
      notes.push(
        "Nothing else could be resolved from here: its calls did not match indexed code (framework callbacks, dynamic dispatch). Its source is in the side panel.",
      );
    this.noteEl.hidden = notes.length === 0;
    this.noteEl.className = "story-note";
    this.noteEl.textContent = notes.join(" ");
    this.render();
    this.fit("width");
  }

  hide(): void {
    this.el.hidden = true;
    this.story = null;
  }

  setSelected(id: string | null): void {
    this.selected = id;
    for (const g of this.viewport.querySelectorAll<SVGGElement>(".step")) {
      g.classList.toggle("selected", g.dataset.node === id);
    }
  }

  private render(): void {
    clear(this.viewport);
    clear(this.lanesEl);
    const story = this.story;
    if (!story) return;
    if (!story.steps.length) {
      this.noteEl.hidden = false;
      this.noteEl.className = "story-note";
      this.noteEl.textContent =
        "Nothing to tell: no calls, requests or effects were found from here. Try an entrypoint from Flows.";
      return;
    }
    const layout = layoutStory(story, this.expanded);
    this.layout = layout;
    const back = svg("g", { class: "lanes-bg" });
    for (const lane of layout.lanes) {
      back.append(
        svg("rect", {
          x: lane.x + 6,
          y: 0,
          width: lane.w - 12,
          height: layout.height,
          rx: 10,
          class: `lane-bg lane-${lane.kind}`,
        }),
        svg("line", {
          x1: lane.cx,
          x2: lane.cx,
          y1: 0,
          y2: layout.height,
          class: "lifeline",
          stroke: LANE_COLORS[lane.kind],
        }),
      );
      const header = h(
        "div",
        { class: "lane-head", dataset: { lane: lane.id } },
        h("i", { style: { background: LANE_COLORS[lane.kind] } }),
        h("b", null, lane.label),
        h("span", { class: "muted small" }, LANE_HINT[lane.kind]),
      );
      header.title = lane.label;
      this.lanesEl.append(header);
    }
    const arrows = svg("g", { class: "arrows" });
    const labels = svg("g", { class: "arrow-labels" });
    for (const a of layout.arrows) this.drawArrow(a, arrows, labels);
    const boxes = svg("g", { class: "steps" });
    for (const b of layout.boxes) boxes.append(this.drawBox(b));
    this.viewport.append(back, arrows, boxes, labels);
    clampTexts(this.viewport);
    this.apply();
  }

  private drawArrow(a: Arrow, arrows: SVGGElement, labels: SVGGElement): void {
    const color = EDGE_COLORS[a.link.kind];
    const d = a.points.map(([x, y], i) => `${i ? "L" : "M"}${x} ${y}`).join(" ");
    const cls = [
      "link",
      a.link.detached ? "detached" : "",
      a.link.confidence === "name" ? "guessed" : "",
    ]
      .filter(Boolean)
      .join(" ");
    const path = svg("path", {
      d,
      class: cls,
      stroke: color,
      "marker-end": `url(#story-arrow-${a.link.kind})`,
      "data-from": a.link.from,
      "data-to": a.link.to,
    });
    path.append(
      svg(
        "title",
        {},
        `${a.link.kind}${a.link.label ? ` · ${a.link.label}` : ""}${a.link.detached ? " · not awaited" : ""} · ${a.link.confidence}`,
      ),
    );
    arrows.append(path);
    if (a.label && a.link.label) {
      labels.append(
        clampable(
          {
            x: a.label.x,
            y: a.label.y,
            "text-anchor": a.label.anchor,
            class: "link-label",
            fill: color,
            "data-from": a.link.from,
            "data-to": a.link.to,
          },
          a.link.label,
          Math.max(40, a.label.maxW),
        ),
      );
    }
  }

  private drawBox(b: StepBox): SVGGElement {
    const s = b.step;
    const color = LANE_COLORS[b.lane.kind];
    const node = this.index.node(s.node);
    const g = svg("g", {
      class: `step${b.compact ? " compact" : ""}${s.node === this.selected ? " selected" : ""}${s.seq === 0 ? " root" : ""}`,
      transform: `translate(${b.x}, ${b.y})`,
      tabindex: 0,
      role: "button",
      "aria-label": `Step ${s.seq + 1}: ${s.label}`,
      "data-node": s.node,
    });
    g.append(svg("title", {}, node?.qualified_name ?? s.node));
    if (b.compact) {
      g.append(
        svg("rect", { width: b.w, height: b.h, rx: b.h / 2, class: "box", stroke: color }),
        svg("text", { x: 14, y: b.h / 2 + 4, class: "seq-inline" }, String(s.seq + 1)),
        clampable({ x: 34, y: b.h / 2 + 4, class: "step-label mono" }, s.label, b.w - 46),
      );
    } else {
      g.append(
        svg("rect", { width: b.w, height: b.h, rx: 8, class: "box", stroke: color }),
        svg("rect", { x: 0, y: 0, width: 4, height: b.h, rx: 2, fill: color }),
        svg("circle", { cx: 20, cy: 19, r: 10, class: "seq", stroke: color }),
        svg(
          "text",
          { x: 20, y: 23, class: "seq-text", "text-anchor": "middle" },
          String(s.seq + 1),
        ),
        clampable({ x: 38, y: 23, class: "step-label" }, s.label, b.w - 46),
      );
      const where = node?.file
        ? `${node.file.split("/").pop()}${node.span ? `:${node.span.start_line}` : ""}`
        : s.kind;
      const helpers = s.folded.length;
      const helperText = helpers
        ? `${b.expanded ? "▾" : "▸"} +${helpers} helper${helpers > 1 ? "s" : ""}`
        : "";
      g.append(
        clampable(
          { x: 38, y: 39, class: "step-sub" },
          where,
          b.w - 46 - (helpers ? textWidth(helperText, 11) + 8 : 0),
        ),
      );
      if (helpers) {
        const toggle = svg(
          "text",
          {
            x: b.w - 8,
            y: 39,
            class: "helpers-toggle",
            "text-anchor": "end",
            tabindex: 0,
            role: "button",
          },
          helperText,
        );
        toggle.append(svg("title", {}, "Show the helper functions folded into this step"));
        toggle.addEventListener("click", (ev) => {
          ev.stopPropagation();
          this.toggleHelpers(s.node);
        });
        g.append(toggle);
      }
      this.drawChips(g, s.effects, b.w);
      if (b.expanded) {
        s.folded.forEach((id, i) => {
          const name = this.index.node(id)?.name ?? shortId(id);
          const t = svg(
            "text",
            {
              x: 38,
              y: BOX_H + 10 + i * HELPER_ROW,
              class: "helper mono",
              tabindex: 0,
              role: "button",
            },
            `· ${fitText(name, b.w - 50, 11)}`,
          );
          t.addEventListener("click", (ev) => {
            ev.stopPropagation();
            this.cb.select(id);
          });
          g.append(t);
        });
      }
    }
    g.addEventListener("click", () => {
      if (this.drag?.moved) return;
      this.cb.select(s.node);
    });
    g.addEventListener("keydown", (ev) => {
      if (ev.key === "Enter" || ev.key === " ") {
        ev.preventDefault();
        this.cb.select(s.node);
      }
    });
    g.addEventListener("mouseenter", () => this.hot(s.node));
    g.addEventListener("mouseleave", () => this.hot(null));
    return g;
  }

  private drawChips(g: SVGGElement, effects: Effect[], w: number): void {
    const seen = new Set<string>();
    const chips = effects.filter((e) => {
      const key = `${e.kind}|${e.target ?? ""}`;
      if (seen.has(key)) return false;
      seen.add(key);
      return true;
    });
    let x = 8;
    const y = 46;
    for (let i = 0; i < chips.length; i++) {
      const e = chips[i]!;
      const rest = chips.length - i - 1;
      const reserve = rest ? 34 : 0;
      const full = effectChip(e);
      const room = w - x - 8 - reserve;
      if (room < 40) {
        g.append(svg("text", { x, y: y + 11, class: "chip-more" }, `+${chips.length - i}`));
        break;
      }
      const text = fitText(full, room - 12, 10.5);
      const cw = textWidth(text, 10.5) + 12;
      const color = EFFECT_COLORS[e.kind];
      const chip = svg("g", { transform: `translate(${x}, ${y})`, class: "fx-chip" });
      chip.append(
        svg("rect", { width: cw, height: 15, rx: 7.5, stroke: color }),
        svg("text", { x: 6, y: 11, fill: color }, text),
        svg(
          "title",
          {},
          `${e.kind}${e.target ? ` ${e.target}` : ""}${e.via ? ` via ${e.via}` : ""}`,
        ),
      );
      g.append(chip);
      x += cw + 4;
    }
  }

  private toggleHelpers(id: string): void {
    if (this.expanded.has(id)) this.expanded.delete(id);
    else this.expanded.add(id);
    this.render();
  }

  private hot(id: string | null): void {
    this.svgEl.classList.toggle("hovering", id !== null);
    for (const el of this.viewport.querySelectorAll<SVGElement>(".link, .link-label")) {
      const on = id !== null && (el.dataset.from === id || el.dataset.to === id);
      el.classList.toggle("hot", on);
    }
  }

  private bindPanZoom(): void {
    this.svgEl.addEventListener(
      "wheel",
      (ev) => {
        ev.preventDefault();
        if (ev.ctrlKey || ev.metaKey) {
          const r = this.svgEl.getBoundingClientRect();
          this.zoomBy(Math.exp(-ev.deltaY * 0.0025), ev.clientX - r.left, ev.clientY - r.top);
        } else {
          this.tx -= ev.shiftKey ? ev.deltaY : ev.deltaX;
          this.ty -= ev.shiftKey ? 0 : ev.deltaY;
          this.apply();
        }
      },
      { passive: false },
    );
    this.svgEl.addEventListener("pointerdown", (ev) => {
      if (ev.button !== 0) return;
      this.drag = { x: ev.clientX, y: ev.clientY, tx: this.tx, ty: this.ty, moved: false };
    });
    window.addEventListener("pointermove", (ev) => {
      const d = this.drag;
      if (!d) return;
      const dx = ev.clientX - d.x;
      const dy = ev.clientY - d.y;
      if (!d.moved && Math.hypot(dx, dy) < 4) return;
      if (!d.moved) this.svgEl.classList.add("panning");
      d.moved = true;
      this.tx = d.tx + dx;
      this.ty = d.ty + dy;
      this.apply();
    });
    window.addEventListener("pointerup", () => {
      this.svgEl.classList.remove("panning");
      setTimeout(() => {
        this.drag = null;
      }, 0);
    });
  }

  zoomBy(f: number, cx?: number, cy?: number): void {
    const r = this.stage.getBoundingClientRect();
    const px = cx ?? r.width / 2;
    const py = cy ?? r.height / 2;
    const k = Math.min(3, Math.max(0.15, this.k * f));
    this.tx = px - ((px - this.tx) * k) / this.k;
    this.ty = py - ((py - this.ty) * k) / this.k;
    this.k = k;
    this.apply();
  }

  /** `width` fits the lanes across and starts at the top; `all` fits the whole diagram. */
  fit(mode: "width" | "all"): void {
    const layout = this.layout;
    if (!layout) return;
    const r = this.stage.getBoundingClientRect();
    const availW = Math.max(200, r.width - 24);
    const availH = Math.max(200, r.height - HEADER_H - 16);
    let k = Math.min(1.15, availW / layout.width);
    if (mode === "width") k = Math.max(k, 0.8);
    else k = Math.min(k, availH / layout.height);
    this.k = Math.max(0.15, k);
    this.tx = Math.max(12, (r.width - layout.width * this.k) / 2);
    this.ty = HEADER_H + 4;
    this.apply();
  }

  private apply(): void {
    this.viewport.setAttribute("transform", `translate(${this.tx}, ${this.ty}) scale(${this.k})`);
    const layout = this.layout;
    if (!layout) return;
    const heads = this.lanesEl.children;
    layout.lanes.forEach((lane, i) => {
      const el = heads[i] as HTMLElement | undefined;
      if (!el) return;
      el.style.left = `${this.tx + (lane.x + 6) * this.k}px`;
      el.style.width = `${(lane.w - 12) * this.k}px`;
    });
  }
}

function clampable(
  attrs: Record<string, string | number>,
  text: string,
  maxW: number,
): SVGTextElement {
  return svg("text", { ...attrs, "data-max": Math.round(maxW) }, text);
}

/** Truncates `text[data-max]` to its measured width with an ellipsis; the full text becomes a tooltip. */
function clampTexts(root: SVGElement): void {
  for (const t of root.querySelectorAll<SVGTextElement>("text[data-max]")) {
    const max = Number(t.dataset.max);
    const full = t.textContent ?? "";
    if (t.getComputedTextLength() <= max) continue;
    let lo = 1;
    let hi = full.length;
    while (lo < hi) {
      const mid = (lo + hi + 1) >> 1;
      t.textContent = `${full.slice(0, mid)}…`;
      if (t.getComputedTextLength() <= max) lo = mid;
      else hi = mid - 1;
    }
    t.textContent = `${full.slice(0, lo)}…`;
    t.append(svg("title", {}, full));
  }
}

function defs(): SVGDefsElement {
  const d = svg("defs", {});
  for (const [kind, color] of Object.entries(EDGE_COLORS)) {
    d.append(
      svg(
        "marker",
        {
          id: `story-arrow-${kind}`,
          viewBox: "0 0 10 10",
          refX: 9,
          refY: 5,
          markerWidth: 8,
          markerHeight: 8,
          markerUnits: "userSpaceOnUse",
          orient: "auto",
        },
        svg("path", { d: "M0 0 L10 5 L0 10 z", fill: color }),
      ),
    );
  }
  return d;
}
