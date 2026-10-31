import type { Store } from "../state";
import type { Insight, TraceSummary } from "../types";
import { clear, fmtMs, fmtNanos, fmtRate, h, shortId } from "./dom";

const INSIGHT_LABELS: Partial<Record<string, string>> = {
  inefficient: "Inefficient code",
  hot_path: "Hot paths",
  waiting: "Waiting on I/O",
  parallelisable: "Parallelisable",
  io_in_loop: "I/O in loop",
  never_observed: "Never observed",
};

export class InsightsPanel {
  readonly el: HTMLElement;
  private readonly body: HTMLElement;

  constructor(
    private readonly store: Store,
    private readonly onPick: (insight: Insight) => void,
  ) {
    this.body = h("div", { class: "panel-body" });
    this.el = h(
      "section",
      { class: "side-list", hidden: true, "aria-label": "Insights" },
      h(
        "div",
        { class: "drawer-head" },
        h("span", { class: "drawer-title" }, "Insights"),
        h("span", { class: "spacer" }),
        h(
          "button",
          {
            class: "ghost small",
            "aria-label": "Close insights",
            onclick: () => this.toggle(false),
          },
          "✕",
        ),
      ),
      this.body,
    );
  }

  async toggle(show = this.el.hidden): Promise<void> {
    this.el.hidden = !show;
    if (!show) return;
    clear(this.body);
    this.body.append(h("div", { class: "muted pad" }, "Loading…"));
    try {
      const { insights } = await this.store.backend.insights();
      this.render(insights);
    } catch (err) {
      clear(this.body);
      this.body.append(h("div", { class: "error pad" }, (err as Error).message));
    }
  }

  private render(insights: Insight[]): void {
    clear(this.body);
    if (!insights.length) {
      this.body.append(
        h(
          "p",
          { class: "muted pad" },
          "No findings yet. Insights appear once the graph is analysed and spans arrive.",
        ),
      );
      return;
    }
    const groups = new Map<Insight["kind"], Insight[]>();
    for (const i of insights) {
      const list = groups.get(i.kind);
      if (list) list.push(i);
      else groups.set(i.kind, [i]);
    }
    for (const [kind, items] of groups) {
      this.body.append(
        h("h3", null, `${INSIGHT_LABELS[kind] ?? kind.replace(/_/g, " ")} (${items.length})`),
      );
      for (const i of items) {
        const name = this.store.index.node(i.node)?.name ?? shortId(i.node);
        this.body.append(
          h(
            "button",
            { class: `insight insight-${i.kind}`, onclick: () => this.onPick(i) },
            h("b", null, name),
            h("span", { class: "muted small" }, describe(i)),
          ),
        );
      }
    }
  }
}

function describe(i: Insight): string {
  const known = describeKnown(i);
  const extra = extraFields(
    Object.fromEntries(Object.entries(i).filter(([k]) => !KNOWN_FIELDS.has(k))),
  );
  return extra ? `${known} · ${extra}` : known;
}

const KNOWN_FIELDS = new Set([
  "kind",
  "node",
  "self_p95_nanos",
  "complexity",
  "reasons",
  "rate_per_sec",
  "p95_nanos",
  "wait_ratio",
  "on",
  "children",
  "saved_nanos",
  "effect",
  "line",
  "observed_calls_per_parent",
]);

function describeKnown(i: Insight): string {
  switch (i.kind) {
    case "inefficient":
      return `self p95 ${fmtNanos(i.self_p95_nanos)} · ${i.complexity} · ${i.reasons.join(", ")}`;
    case "hot_path":
      return `${fmtRate(i.rate_per_sec)} · p95 ${fmtNanos(i.p95_nanos)}`;
    case "waiting":
      return `${Math.round(i.wait_ratio * 100)}% waiting on ${i.on.map(shortId).join(", ")}`;
    case "parallelisable":
      return `${i.children.length} sequential children · save ~${fmtNanos(i.saved_nanos)}`;
    case "io_in_loop":
      return `${i.effect} at L${i.line}${i.observed_calls_per_parent ? ` · ${i.observed_calls_per_parent.toFixed(1)} calls/parent` : ""}`;
    case "never_observed":
      return "reachable statically, never seen at runtime";
    default:
      return extraFields(i);
  }
}

/** Unknown insight kinds and fields are shown as `key: value` so new backend findings stay visible. */
function extraFields(i: object): string {
  return Object.entries(i)
    .filter(([k]) => k !== "kind" && k !== "node")
    .map(
      ([k, v]) =>
        `${k}: ${typeof v === "number" ? (k.endsWith("_nanos") ? fmtNanos(v) : v.toFixed(2)) : Array.isArray(v) ? v.map((x) => (typeof x === "string" ? shortId(x) : String(x))).join(", ") : String(v)}`,
    )
    .join(" · ");
}

export class TracesPanel {
  readonly el: HTMLElement;
  private readonly body: HTMLElement;

  constructor(
    private readonly store: Store,
    private readonly onPick: (trace: TraceSummary) => void,
  ) {
    this.body = h("div", { class: "panel-body" });
    this.el = h(
      "section",
      { class: "side-list", hidden: true, "aria-label": "Recent traces" },
      h(
        "div",
        { class: "drawer-head" },
        h("span", { class: "drawer-title" }, "Recent traces"),
        h("span", { class: "spacer" }),
        h("button", { class: "ghost small", onclick: () => this.refresh() }, "↻"),
        h(
          "button",
          { class: "ghost small", "aria-label": "Close traces", onclick: () => this.toggle(false) },
          "✕",
        ),
      ),
      this.body,
    );
    store.on("traces", (traces) => this.render(traces));
  }

  async toggle(show = this.el.hidden): Promise<void> {
    this.el.hidden = !show;
    if (show) await this.refresh();
  }

  async refresh(): Promise<void> {
    try {
      this.store.traces = await this.store.backend.traces(50);
      this.render(this.store.traces);
    } catch (err) {
      clear(this.body);
      this.body.append(h("div", { class: "error pad" }, (err as Error).message));
    }
  }

  private render(traces: TraceSummary[]): void {
    if (this.el.hidden) return;
    clear(this.body);
    if (!traces.length) {
      this.body.append(
        h("p", { class: "muted pad" }, "No traces yet. Point an OTLP exporter at the server."),
      );
      return;
    }
    for (const t of traces) {
      const t0 = Math.min(...t.spans.map((s) => s.start_unix_nanos));
      const t1 = Math.max(...t.spans.map((s) => s.end_unix_nanos));
      const errors = t.spans.filter((s) => s.error).length;
      this.body.append(
        h(
          "button",
          { class: `trace${errors ? " err" : ""}`, onclick: () => this.onPick(t) },
          h("b", null, t.root),
          h(
            "span",
            { class: "muted small" },
            `${t.spans[0]?.service ?? ""} · ${fmtMs((t1 - t0) / 1e6)} · ${t.spans.length} spans${errors ? ` · ${errors} errors` : ""}`,
          ),
          h(
            "span",
            { class: "muted small mono" },
            `${t.trace_id} · ${new Date(t0 / 1e6).toLocaleTimeString()}`,
          ),
        ),
      );
    }
  }
}
