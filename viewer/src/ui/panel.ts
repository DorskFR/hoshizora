import { EFFECT_COLORS, EFFECT_ICONS } from "../graph/colors";
import type { Store } from "../state";
import type { Edge, NodeDetail, NodeStats } from "../types";
import { clear, fmtNanos, fmtRate, h, shortId } from "./dom";
import { highlight, shikiLang } from "./highlight";

export interface PanelActions {
  story(id: string): void;
  impact(id: string): void;
  browseFlows(): void;
}

const SECTIONS_KEY = "hoshizora.sections";
const DEFAULT_OPEN: Record<string, boolean> = {
  effects: true,
  source: true,
  signature: false,
  runtime: false,
  edges: false,
};

function sectionState(): Record<string, boolean> {
  try {
    return { ...DEFAULT_OPEN, ...JSON.parse(localStorage.getItem(SECTIONS_KEY) ?? "{}") };
  } catch {
    return { ...DEFAULT_OPEN };
  }
}

/** A collapsible panel section whose open state is remembered across nodes and reloads. */
function section(key: string, title: string | Node, ...body: (Node | false | null)[]): HTMLElement {
  const details = h(
    "details",
    { class: `section section-${key}` },
    h("summary", null, title),
    ...body,
  );
  details.open = sectionState()[key] ?? true;
  details.addEventListener("toggle", () => {
    const state = sectionState();
    state[key] = details.open;
    localStorage.setItem(SECTIONS_KEY, JSON.stringify(state));
  });
  return details;
}

const STORY_KINDS = new Set(["function", "route", "type"]);

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
    const close = h(
      "button",
      { class: "ghost panel-close", "aria-label": "Close", onclick: () => store.select(null) },
      "×",
    );
    this.el = h("section", { class: "node-panel", "aria-label": "Node details" }, close, this.body);
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
        h("p", { class: "lead" }, "What happens when someone clicks a button?"),
        h(
          "button",
          { class: "primary big", onclick: () => this.actions.browseFlows() },
          "▶ Browse flows",
        ),
        h(
          "p",
          { class: "muted" },
          "Pick a UI action, HTTP route or command to see its story: a step-by-step diagram across the UI, services, workers and databases.",
        ),
        h(
          "p",
          { class: "muted small" },
          "Or search ( / ) for a symbol, or click a star in the map. Double-click a package to open it.",
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
          this.store.expanded.has(id) ? "Collapse in map" : "Expand in map",
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
    const route = this.store.index.in.get(n.id)?.find((e) => e.kind === "handles");

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
          route && h("span", { class: "chip mono" }, this.store.index.node(route.from)?.name ?? ""),
        ),
        h("h2", null, n.name),
        n.file &&
          h(
            "div",
            { class: "muted mono wrap small" },
            `${n.file}${n.span ? `:${n.span.start_line}–${n.span.end_line}` : ""}`,
          ),
      ),
      h(
        "div",
        { class: "actions" },
        STORY_KINDS.has(n.kind) &&
          h(
            "button",
            {
              class: "primary",
              onclick: () => this.actions.story(n.id),
              title: "Step-by-step diagram of what this triggers (s)",
            },
            "▶ Story",
          ),
        h(
          "button",
          { onclick: () => this.actions.impact(n.id), title: "i" },
          "Impact of changing this",
        ),
        h("button", { onclick: () => this.copyQuery(n.id) }, "Copy agent query"),
        editorHref && h("a", { class: "button", href: editorHref }, "Open in editor"),
      ),
    );
    if (f?.doc) this.body.append(h("p", { class: "doc" }, f.doc));

    this.body.append(this.renderEffects(n));

    if (d.source) {
      const text = dedent(d.source.text);
      const code = h("div", { class: "code" }, h("pre", { class: "plain-code mono" }, text));
      this.body.append(
        section(
          "source",
          h(
            "span",
            null,
            "Source ",
            h(
              "span",
              { class: "muted mono small" },
              `${d.source.file.split("/").pop()}:${d.source.start_line}`,
            ),
          ),
          code,
        ),
      );
      const token = this.token;
      highlight(text, shikiLang(n.language, n.file), d.source.start_line)
        .then((html) => {
          if (token !== this.token) return;
          code.innerHTML = html;
        })
        .catch(() => undefined);
    }

    if (f) {
      this.body.append(
        section(
          "signature",
          "Signature & complexity",
          h("pre", { class: "signature mono" }, f.signature || n.qualified_name),
          f.params.length > 0 &&
            h(
              "table",
              { class: "params" },
              h(
                "thead",
                null,
                h(
                  "tr",
                  null,
                  h("th", null, "name"),
                  h("th", null, "type"),
                  h("th", null, "default"),
                ),
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
              h(
                "span",
                { class: "mono" },
                `${f.complexity.time} time · ${f.complexity.space} space`,
              ),
              f.complexity.basis.length > 0 &&
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
        ),
      );
    }

    this.body.append(
      section("runtime", "Runtime", this.renderStats(stats)),
      section(
        "edges",
        `Callers & callees (${d.incoming.filter((e) => e.kind !== "contains").length} / ${d.outgoing.filter((e) => e.kind !== "contains").length})`,
        this.renderEdges("Called by", d.incoming, (e) => e.from),
        this.renderEdges("Calls", d.outgoing, (e) => e.to),
      ),
    );
  }

  private renderEffects(n: NodeDetail["node"]): HTMLElement {
    const body: HTMLElement[] = [];
    if (n.effects.direct.length === 0 && n.effects.transitive.length === 0) {
      body.push(h("p", { class: "muted small" }, "Pure: no I/O, processes or locks detected."));
    }
    if (n.effects.direct.length) {
      body.push(
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
                `${EFFECT_ICONS[e.kind]} ${e.kind.replace("_", " ")}`,
              ),
              e.target && h("span", { class: "mono" }, ` ${e.target}`),
              h(
                "span",
                { class: "muted small" },
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
      body.push(
        h(
          "div",
          { class: "chips" },
          h("span", { class: "muted small" }, "through callees: "),
          ...transitiveOnly.map((k) =>
            h(
              "span",
              { class: "chip", style: { borderColor: EFFECT_COLORS[k] } },
              `${EFFECT_ICONS[k]} ${k.replace("_", " ")}`,
            ),
          ),
        ),
      );
    }
    return section("effects", "Effects", ...body);
  }

  private renderStats(stats: NodeStats | null): HTMLElement {
    const box = h("div", { class: "stats-box" });
    if (!stats) {
      box.append(h("p", { class: "muted small" }, "No spans observed yet."));
      return box;
    }
    const waitPct = stats.total_nanos
      ? Math.round((stats.wait_nanos / stats.total_nanos) * 100)
      : 0;
    box.append(
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
      h("h4", null, `${title} (${shown.length})`),
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
      this.store.mode === "impact"
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

/** Strips the indentation shared by every non-blank line, so nested code starts at column 0. */
export function dedent(text: string): string {
  const lines = text.split("\n");
  let min = Number.POSITIVE_INFINITY;
  for (const l of lines) {
    if (!l.trim()) continue;
    min = Math.min(min, l.length - l.trimStart().length);
  }
  if (!Number.isFinite(min) || min === 0) return text;
  return lines.map((l) => l.slice(Math.min(min, l.length - l.trimStart().length))).join("\n");
}

function shellQuote(s: string): string {
  return `'${s.replace(/'/g, `'\\''`)}'`;
}
