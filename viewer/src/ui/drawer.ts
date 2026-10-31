import { IMPACT } from "../graph/colors";
import { waterfall } from "../runtime/tracker";
import type { Store } from "../state";
import type { Impact, TraceSummary } from "../types";
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
    this.title.textContent = `Trace ${trace.trace_id} · ${trace.root} · ${fmtMs(wf.totalMs)} · ${trace.spans.length} spans`;
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
