import {
  EDGE_COLORS,
  EFFECT_COLORS,
  EFFECT_ICONS,
  IMPACT,
  SYNTHETIC_COLORS,
} from "../graph/colors";
import { layoutFlow } from "../graph/flowLayout";
import { waterfall } from "../runtime/tracker";
import type { Store } from "../state";
import type { Impact, Subgraph, TraceSummary } from "../types";
import { clear, fmtMs, h, shortId } from "./dom";

const SVG = "http://www.w3.org/2000/svg";

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

export class Drawer {
  readonly el: HTMLElement;
  private readonly title: HTMLElement;
  private readonly body: HTMLElement;
  private readonly extra: HTMLElement;

  constructor(private readonly store: Store) {
    this.title = h("span", { class: "drawer-title" });
    this.extra = h("span", { class: "drawer-extra" });
    this.body = h("div", { class: "drawer-body" });
    this.el = h(
      "section",
      { class: "drawer", hidden: true, "aria-label": "Flow panel" },
      h(
        "div",
        { class: "drawer-head" },
        this.title,
        this.extra,
        h("span", { class: "spacer" }),
        h(
          "button",
          { class: "ghost small", "aria-label": "Close panel", onclick: () => this.hide() },
          "✕",
        ),
      ),
      this.body,
    );
  }

  hide(): void {
    this.el.hidden = true;
    document.body.classList.remove("drawer-open");
    clear(this.extra);
  }

  private open(): void {
    this.el.hidden = false;
    document.body.classList.add("drawer-open");
  }

  showFlow(flow: Subgraph): void {
    this.open();
    clear(this.extra);
    const rootName = this.store.index.node(flow.root)?.name ?? shortId(flow.root);
    this.title.textContent = `Flow from ${rootName} · ${flow.nodes.length} nodes${flow.truncated ? " · truncated" : ""}`;
    clear(this.body);
    const layout = layoutFlow(flow.steps);
    if (!layout.boxes.length) {
      this.body.append(h("p", { class: "muted pad" }, "Nothing reachable."));
      return;
    }
    const colW = 150;
    const rowH = 64;
    const boxW = 124;
    const boxH = 40;
    const laneW = 110;
    const width = laneW + layout.cols * colW + 20;
    const height = layout.lanes.length * rowH + 12;
    const root = svg("svg", {
      width,
      height,
      class: "flow-svg",
      role: "img",
      "aria-label": "Flow swimlanes",
    });
    const defs = svg("defs", {});
    defs.append(
      svg(
        "marker",
        {
          id: "arrow",
          viewBox: "0 0 10 10",
          refX: 9,
          refY: 5,
          markerWidth: 7,
          markerHeight: 7,
          orient: "auto-start-reverse",
        },
        svg("path", { d: "M0 0 L10 5 L0 10 z", fill: "#9aa5c4" }),
      ),
    );
    root.append(defs);
    layout.lanes.forEach((lane, r) => {
      root.append(
        svg("rect", {
          x: 0,
          y: r * rowH + 6,
          width,
          height: rowH - 2,
          class: `lane ${r % 2 ? "alt" : ""}`,
        }),
      );
      root.append(svg("text", { x: 10, y: r * rowH + rowH / 2 + 8, class: "lane-label" }, lane));
    });
    const center = (b: { col: number; row: number }) => ({
      x: laneW + b.col * colW + boxW / 2,
      y: b.row * rowH + 6 + rowH / 2,
    });
    for (const a of layout.arrows) {
      const from = center(layout.boxes[a.from]!);
      const to = center(layout.boxes[a.to]!);
      const x1 = from.x + boxW / 2;
      const x2 = to.x - boxW / 2;
      const mx = (x1 + x2) / 2;
      const d = `M${x1} ${from.y} C ${mx} ${from.y}, ${mx} ${to.y}, ${x2} ${to.y}`;
      root.append(
        svg("path", {
          d,
          class: "flow-arrow",
          stroke: EDGE_COLORS[a.kind],
          "marker-end": "url(#arrow)",
        }),
      );
      root.append(
        svg(
          "text",
          {
            x: mx,
            y: (from.y + to.y) / 2 - 6,
            class: "arrow-label",
            fill: EDGE_COLORS[a.kind],
            "text-anchor": "middle",
          },
          a.kind,
        ),
      );
    }
    for (const b of layout.boxes) {
      const n = this.store.index.node(b.node);
      const c = center(b);
      const color = n
        ? (SYNTHETIC_COLORS[n.kind] ??
          (n.effects.direct[0] ? EFFECT_COLORS[n.effects.direct[0].kind] : "#5b6b95"))
        : "#5b6b95";
      const g = svg("g", {
        class: `flow-box${this.store.selected === b.node ? " selected" : ""}`,
        transform: `translate(${c.x - boxW / 2}, ${c.y - boxH / 2})`,
        tabindex: 0,
        role: "button",
      });
      g.append(svg("rect", { width: boxW, height: boxH, rx: 6, stroke: color }));
      g.append(
        svg("text", { x: 8, y: 17, class: "box-name" }, truncate(n?.name ?? shortId(b.node), 16)),
      );
      const icons = n
        ? Array.from(new Set(n.effects.direct.map((e) => e.kind)))
            .map((k) => EFFECT_ICONS[k])
            .join(" ")
        : "";
      g.append(
        svg(
          "text",
          { x: 8, y: 32, class: "box-sub", fill: color },
          `${n?.kind ?? ""}${icons ? `  ${icons}` : ""}${n?.dead ? "  dead" : ""}`,
        ),
      );
      g.append(svg("title", {}, n?.qualified_name ?? b.node));
      g.addEventListener("click", () => this.store.select(b.node));
      g.addEventListener("keydown", (ev) => {
        if (ev.key === "Enter" || ev.key === " ") this.store.select(b.node);
      });
      root.append(g);
    }
    this.body.append(h("div", { class: "scroll-x" }, root));
  }

  showImpact(impact: Impact): void {
    this.open();
    clear(this.extra);
    const name = this.store.index.node(impact.target)?.name ?? shortId(impact.target);
    this.title.textContent = `Impact of changing ${name} · ${impact.affected.length} affected · ${impact.entrypoints.length} entrypoints`;
    clear(this.body);
    if (!impact.entrypoints.length) {
      this.body.append(
        h(
          "p",
          { class: "muted pad" },
          impact.affected.length
            ? "Affected code is not reachable from any entrypoint."
            : "Nothing depends on this node.",
        ),
      );
      return;
    }
    const groups = new Map<string, typeof impact.entrypoints>();
    for (const e of impact.entrypoints) {
      const list = groups.get(e.kind);
      if (list) list.push(e);
      else groups.set(e.kind, [e]);
    }
    const list = h("div", { class: "impact-list" });
    for (const [kind, entries] of groups) {
      list.append(
        h("h4", { style: { color: IMPACT } }, `${kind.replace("_", " ")} (${entries.length})`),
      );
      for (const e of entries) {
        const path = h("div", { class: "path" });
        e.path.forEach((id, i) => {
          if (i > 0) path.append(h("span", { class: "muted" }, " → "));
          path.append(
            h(
              "button",
              { class: "link mono", onclick: () => this.store.select(id) },
              this.store.index.node(id)?.name ?? shortId(id),
            ),
          );
        });
        list.append(
          h(
            "div",
            { class: "impact-entry" },
            h(
              "button",
              { class: "link strong", onclick: () => this.store.select(e.node) },
              this.store.index.node(e.node)?.name ?? shortId(e.node),
            ),
            path,
          ),
        );
      }
    }
    this.body.append(list);
  }

  showWaterfall(trace: TraceSummary, controls: HTMLElement | null): void {
    this.open();
    clear(this.extra);
    if (controls) this.extra.append(controls);
    const wf = waterfall(trace.spans);
    this.title.textContent = `Trace ${trace.trace_id} · ${trace.root.name} · ${fmtMs(wf.totalMs)} · ${trace.spans.length} spans`;
    clear(this.body);
    const labelW = 240;
    const rowH = 22;
    const width = Math.max(600, this.el.clientWidth - 40);
    const barsW = width - labelW - 80;
    const root = svg("svg", {
      width,
      height: wf.rows.length * rowH + 10,
      class: "wf-svg",
      role: "img",
      "aria-label": "Trace waterfall",
    });
    wf.rows.forEach((row, i) => {
      const y = i * rowH + 4;
      const x = labelW + (row.offsetMs / wf.totalMs) * barsW;
      const w = Math.max(2, (row.durationMs / wf.totalMs) * barsW);
      const node = row.span.node ? this.store.index.node(row.span.node) : null;
      const color = row.span.error
        ? "#f94144"
        : row.span.kind === "client"
          ? "#4cc9f0"
          : row.span.kind === "server"
            ? "#48bfe3"
            : "#c77dff";
      const g = svg("g", { class: "wf-row", tabindex: 0, role: "button" });
      g.append(
        svg(
          "text",
          { x: 8 + row.depth * 12, y: y + 15, class: "wf-label" },
          truncate(`${row.span.service} · ${node?.name ?? row.span.name}`, 32 - row.depth),
        ),
      );
      g.append(
        svg("rect", { x, y: y + 4, width: w, height: rowH - 8, rx: 3, fill: color, opacity: 0.9 }),
      );
      if (row.waitRatio > 0) {
        g.append(
          svg("rect", {
            x: x + w * (1 - row.waitRatio),
            y: y + 4,
            width: w * row.waitRatio,
            height: rowH - 8,
            rx: 3,
            class: "wf-wait",
          }),
        );
      }
      g.append(svg("text", { x: x + w + 6, y: y + 15, class: "wf-dur" }, fmtMs(row.durationMs)));
      g.append(
        svg(
          "title",
          {},
          `${row.span.name}\n${fmtMs(row.durationMs)} · waiting ${(row.waitRatio * 100).toFixed(0)}%`,
        ),
      );
      if (row.span.node) {
        const id = row.span.node;
        g.addEventListener("click", () => this.store.select(id));
        g.addEventListener("keydown", (ev) => {
          if (ev.key === "Enter") this.store.select(id);
        });
      }
      root.append(g);
    });
    this.body.append(
      h("div", { class: "scroll-x" }, root),
      h("div", { class: "muted small pad" }, "Solid: self time · hatched: waiting on child spans"),
    );
  }
}

function truncate(s: string, n: number): string {
  return s.length > n ? `${s.slice(0, n - 1)}…` : s;
}
