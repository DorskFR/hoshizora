import { EFFECT_COLORS, EFFECT_ICONS } from "../graph/colors";
import type { Store } from "../state";
import type { Edge, NodeDetail, NodeStats } from "../types";
import { clear, fmtNanos, fmtRate, h, shortId } from "./dom";
import { highlight, shikiLang } from "./highlight";

export interface PanelActions {
  flow(id: string): void;
  impact(id: string): void;
}

export class NodePanel {
  readonly el: HTMLElement;
  private readonly body: HTMLElement;
  private current: NodeDetail | null = null;
  private token = 0;

  constructor(
    private readonly store: Store,
    private readonly actions: PanelActions,
  ) {
    this.body = h("div", { class: "panel-body" });
    this.el = h("section", { class: "node-panel", "aria-label": "Node details" }, this.body);
    this.renderEmpty();
    store.on("selection", (id) => this.load(id));
    store.on("stats", () => this.refreshStats());
  }

  private renderEmpty(): void {
    clear(this.body);
    this.body.append(
      h(
        "div",
        { class: "empty" },
        h("p", null, "Click a node to inspect it."),
        h(
          "p",
          { class: "muted" },
          "Double-click a package or file to expand it. Keys: / search, f flow, i impact, Esc clear.",
        ),
      ),
    );
  }

  private async load(id: string | null): Promise<void> {
    const token = ++this.token;
    if (!id) {
      this.current = null;
      this.renderEmpty();
      return;
    }
    const view = this.store.index.node(id);
    if (view?.kind === "package" || view?.kind === "file") {
      this.renderContainer(id);
      return;
    }
    clear(this.body);
    this.body.append(h("div", { class: "muted pad" }, "Loading…"));
    try {
      const detail = await this.store.backend.node(id);
      if (token !== this.token) return;
      this.current = detail;
      this.render(detail);
    } catch (err) {
      if (token !== this.token) return;
      clear(this.body);
      this.body.append(
        h("div", { class: "error pad" }, `Could not load node: ${(err as Error).message}`),
      );
    }
  }

  private renderContainer(id: string): void {
    const n = this.store.index.node(id)!;
    const children = this.store.index.children.get(id) ?? [];
    clear(this.body);
    this.body.append(
      h(
        "header",
        { class: "node-head" },
        h("span", { class: `kind kind-${n.kind}` }, n.kind),
        h("h2", null, n.name),
        h("div", { class: "muted mono" }, n.qualified_name),
      ),
      h(
        "div",
        { class: "actions" },
        h(
          "button",
          { onclick: () => this.store.toggleExpanded(id) },
          this.store.expanded.has(id) ? "Collapse" : "Expand",
        ),
      ),
      h("h3", null, `${children.length} children`),
      h(
        "ul",
        { class: "plain" },
        ...children.slice(0, 200).map((c) => {
          const child = this.store.index.node(c)!;
          return h(
            "li",
            null,
            h(
              "button",
              { class: "link", onclick: () => this.store.select(c) },
              `${child.kind} · ${child.name}`,
            ),
          );
        }),
      ),
    );
  }

  private render(d: NodeDetail): void {
    const n = d.node;
    const f = n.function;
    clear(this.body);
    const root = this.store.graph.meta.root.replace(/\/$/, "");
    const editorHref = n.file ? `vscode://file/${root}/${n.file}:${n.span?.start_line ?? 1}` : null;
    const stats = d.stats ?? this.store.stats.get(n.id) ?? null;

    this.body.append(
      h(
        "header",
        { class: "node-head" },
        h(
          "div",
          { class: "row" },
          h("span", { class: `kind kind-${n.kind}` }, n.kind),
          n.entrypoint &&
            h("span", { class: "chip entry" }, `entrypoint · ${n.entrypoint.replace("_", " ")}`),
          n.dead && h("span", { class: "chip dead" }, "dead"),
        ),
        h("h2", null, n.name),
        h("div", { class: "muted mono wrap" }, n.qualified_name),
        n.file &&
          h(
            "div",
            { class: "muted mono" },
            `${n.file}${n.span ? `:${n.span.start_line}–${n.span.end_line}` : ""}`,
          ),
      ),
      h(
        "div",
        { class: "actions" },
        h("button", { onclick: () => this.actions.flow(n.id), title: "f" }, "Flow from here"),
        h(
          "button",
          { onclick: () => this.actions.impact(n.id), title: "i" },
          "Impact of changing this",
        ),
        h("button", { onclick: () => this.copyQuery(n.id) }, "Copy as agent query"),
        editorHref && h("a", { class: "button", href: editorHref }, "Open in editor"),
      ),
    );

    if (f) {
      this.body.append(h("pre", { class: "signature mono" }, f.signature));
      if (f.doc) this.body.append(h("p", { class: "doc" }, f.doc));
      if (f.params.length) {
        this.body.append(
          h("h3", null, "Parameters"),
          h(
            "table",
            { class: "params" },
            h(
              "thead",
              null,
              h("tr", null, h("th", null, "name"), h("th", null, "type"), h("th", null, "default")),
            ),
            h(
              "tbody",
              null,
              ...f.params.map((p) =>
                h(
                  "tr",
                  null,
                  h("td", { class: "mono" }, p.name),
                  h("td", { class: "mono muted" }, p.ty ?? "—"),
                  h("td", { class: "mono muted" }, p.default ?? "—"),
                ),
              ),
            ),
          ),
        );
      }
      this.body.append(
        h(
          "dl",
          { class: "facts" },
          h("dt", null, "returns"),
          h("dd", { class: "mono" }, f.returns ?? "—"),
          h("dt", null, "flags"),
          h(
            "dd",
            null,
            [
              f.is_async && "async",
              f.is_method && "method",
              f.recursive && "recursive",
              f.visibility,
            ]
              .filter(Boolean)
              .join(" · "),
          ),
          f.attributes.length > 0 && h("dt", null, "attributes"),
          f.attributes.length > 0 &&
            h("dd", { class: "mono" }, f.attributes.map((a) => `@${a}`).join(" ")),
          h("dt", null, "complexity"),
          h(
            "dd",
            null,
            h("span", { class: "mono" }, `${f.complexity.time} time · ${f.complexity.space} space`),
            h("div", { class: "muted small" }, f.complexity.basis.join("; ")),
          ),
          h("dt", null, "cyclomatic"),
          h(
            "dd",
            null,
            `${f.cyclomatic} · ${f.branches} branches · ${f.loops} loops (depth ${f.max_loop_depth})`,
          ),
          h("dt", null, "fan"),
          h("dd", null, `in ${f.fan_in} · out ${f.fan_out}`),
        ),
      );
    }

    this.body.append(h("h3", null, "Effects"));
    if (n.effects.direct.length === 0 && n.effects.transitive.length === 0) {
      this.body.append(h("p", { class: "muted" }, "none"));
    } else {
      if (n.effects.direct.length) {
        this.body.append(
          h(
            "ul",
            { class: "effects" },
            ...n.effects.direct.map((e) =>
              h(
                "li",
                null,
                h(
                  "span",
                  { class: "fx", style: { color: EFFECT_COLORS[e.kind] } },
                  `${EFFECT_ICONS[e.kind]} ${e.kind}`,
                ),
                e.target && h("span", { class: "mono" }, ` ${e.target}`),
                h(
                  "span",
                  { class: "muted" },
                  ` via ${e.via} · L${e.line}${e.loop_depth > 0 ? ` · in loop ×${e.loop_depth}` : ""}${e.awaited ? " · awaited" : ""}`,
                ),
              ),
            ),
          ),
        );
      }
      const transitiveOnly = n.effects.transitive.filter(
        (k) => !n.effects.direct.some((e) => e.kind === k),
      );
      if (transitiveOnly.length) {
        this.body.append(
          h(
            "div",
            { class: "chips" },
            h("span", { class: "muted small" }, "transitive: "),
            ...transitiveOnly.map((k) =>
              h(
                "span",
                { class: "chip", style: { borderColor: EFFECT_COLORS[k] } },
                `${EFFECT_ICONS[k]} ${k}`,
              ),
            ),
          ),
        );
      }
    }

    this.body.append(this.renderStats(stats));
    if (d.source) {
      const code = h(
        "div",
        { class: "code" },
        h("pre", { class: "plain-code mono" }, d.source.text),
      );
      this.body.append(h("h3", null, `Source · ${d.source.file}:${d.source.start_line}`), code);
      const token = this.token;
      highlight(d.source.text, shikiLang(n.language, n.file), d.source.start_line)
        .then((html) => {
          if (token !== this.token) return;
          code.innerHTML = html;
        })
        .catch(() => undefined);
    }
    this.body.append(
      this.renderEdges("Incoming", d.incoming, (e) => e.from),
      this.renderEdges("Outgoing", d.outgoing, (e) => e.to),
    );
  }

  private renderStats(stats: NodeStats | null): HTMLElement {
    const box = h("div", { class: "stats-box" });
    if (!stats) {
      box.append(h("h3", null, "Runtime"), h("p", { class: "muted" }, "no spans observed"));
      return box;
    }
    const waitPct = stats.total_nanos
      ? Math.round((stats.wait_nanos / stats.total_nanos) * 100)
      : 0;
    box.append(
      h("h3", null, "Runtime"),
      h(
        "div",
        { class: "stat-grid" },
        stat("rate", fmtRate(stats.rate_per_sec)),
        stat("count", String(stats.count)),
        stat("errors", String(stats.errors)),
        stat("p50", fmtNanos(stats.p50_nanos)),
        stat("p95", fmtNanos(stats.p95_nanos)),
        stat("p99", fmtNanos(stats.p99_nanos)),
        stat("self", fmtNanos(stats.count ? stats.self_nanos / stats.count : 0)),
        stat("waiting", `${waitPct}%`),
        stat("max", fmtNanos(stats.max_nanos)),
      ),
    );
    return box;
  }

  private refreshStats(): void {
    if (!this.current) return;
    const stats = this.store.stats.get(this.current.node.id) ?? this.current.stats;
    const old = this.body.querySelector(".stats-box");
    if (old) old.replaceWith(this.renderStats(stats));
  }

  private renderEdges(title: string, edges: Edge[], other: (e: Edge) => string): HTMLElement {
    const shown = edges.filter((e) => e.kind !== "contains");
    if (!shown.length) return h("div");
    return h(
      "div",
      null,
      h("h3", null, `${title} (${shown.length})`),
      h(
        "ul",
        { class: "edges" },
        ...shown
          .slice(0, 60)
          .map((e) =>
            h(
              "li",
              null,
              h("span", { class: `ek ek-${e.kind}` }, e.kind),
              h(
                "button",
                { class: "link mono", onclick: () => this.store.select(other(e)) },
                shortId(other(e)),
              ),
              h(
                "span",
                { class: "muted small" },
                ` ${e.confidence}${e.line ? ` · L${e.line}` : ""}${e.loop_depth ? " · loop" : ""}`,
              ),
            ),
          ),
      ),
    );
  }

  private async copyQuery(id: string): Promise<void> {
    const cmd =
      this.store.mode === "flow"
        ? `hoshizora query flow --from ${shellQuote(id)}`
        : this.store.mode === "impact"
          ? `hoshizora query impact --of ${shellQuote(id)}`
          : `hoshizora query node ${shellQuote(id)}`;
    try {
      await navigator.clipboard.writeText(cmd);
      this.store.status(`Copied: ${cmd}`);
    } catch {
      this.store.status(cmd);
    }
  }
}

function stat(label: string, value: string): HTMLElement {
  return h(
    "div",
    { class: "stat" },
    h("span", { class: "muted small" }, label),
    h("b", null, value),
  );
}

function shellQuote(s: string): string {
  return `'${s.replace(/'/g, `'\\''`)}'`;
}
