import "./styles.css";
import { createBackend, isMockMode } from "./api";
import { FLOW, IMPACT } from "./graph/colors";
import { buildView, containersToExpand } from "./graph/lod";
import { GraphIndex } from "./graph/query";
import { Scene } from "./graph/scene";
import { SpanTracker, scheduleReplay } from "./runtime/tracker";
import { type HeatMetric, Store } from "./state";
import type { Insight, LiveEvent, NodeStats, TraceSummary } from "./types";
import { h } from "./ui/dom";
import { Drawer } from "./ui/drawer";
import { Legend } from "./ui/legend";
import { InsightsPanel, TracesPanel } from "./ui/lists";
import { NodePanel } from "./ui/panel";
import { Toolbar } from "./ui/toolbar";

async function main(): Promise<void> {
  const app = document.getElementById("app")!;
  const backend = await createBackend();
  const store = new Store(backend);
  const canvasHost = h("div", { class: "canvas" });
  const splash = h(
    "div",
    { class: "splash" },
    h("span", { class: "kanji" }, "星空"),
    h("span", null, "loading graph…"),
  );
  app.append(canvasHost, splash);

  try {
    store.graph = await backend.graph();
  } catch (err) {
    splash.replaceChildren(
      h("span", { class: "kanji" }, "星空"),
      h("span", { class: "error" }, `Cannot load /api/graph: ${(err as Error).message}`),
      h("span", { class: "muted" }, "Run `hoshizora serve`, or open with ?mock for fixtures."),
    );
    return;
  }
  store.index = new GraphIndex(store.graph);
  splash.remove();

  const scene = new Scene(canvasHost, {
    onClick: (id) => store.select(id),
    onDoubleClick: (id) => {
      const n = store.index.node(id);
      if (n && (n.kind === "package" || n.kind === "file")) store.toggleExpanded(id);
      else store.select(id);
    },
    onBackground: () => store.select(null),
  });

  const params = new URLSearchParams(location.search);
  const level = params.get("level");
  if (level === "files" || level === "functions" || level === "packages") store.level = level;
  const maxLinks = Number(params.get("maxLinks"));
  if (maxLinks > 0) store.maxLinks = maxLinks;

  const tracker = new SpanTracker();
  let stopStream: (() => void) | null = null;
  let replayTimers: number[] = [];
  let flowTimers: number[] = [];

  const drawer = new Drawer(store);
  const panel = new NodePanel(store, {
    flow: (id) => startFlow(id),
    impact: (id) => startImpact(id),
  });
  const insights = new InsightsPanel(store, (i) => pickInsight(i));
  const traces = new TracesPanel(store, (t) => replayTrace(t));
  const legend = new Legend(store, (mode) => scene.setColorBy(mode));
  const toolbar = new Toolbar(store, {
    runtime: (on) => setRuntime(on),
    heat: (metric) => setHeat(metric),
    togglePanel: (name) => (name === "insights" ? insights.toggle() : traces.toggle()),
    zoomToFit: () => scene.zoomToFit(),
  });
  app.append(
    toolbar.el,
    h("div", { class: "right" }, panel.el, insights.el, traces.el),
    legend.el,
    drawer.el,
  );

  let buildCache = { repOf: new Map<string, string>() };
  const rep = (id: string): string | null => buildCache.repOf.get(id) ?? null;

  function rebuildView(): void {
    const t = performance.now();
    const view = buildView(store.index, {
      level: store.level,
      expanded: store.expanded,
      maxLinks: store.maxLinks,
      effectFilter: store.effectFilter,
      transitive: store.transitive,
    });
    buildCache = { repOf: view.repOf };
    scene.setView(view);
    applyHighlight();
    applyHeat();
    scene.setSelected(store.selected ? rep(store.selected) : null);
    const ms = Math.round(performance.now() - t);
    const b = scene.batched;
    const mode = b.nodes
      ? " · batched (no picking: expand a file or search)"
      : b.links
        ? " · batched links"
        : "";
    store.status(
      `${view.nodes.length} nodes · ${view.links.length} links${view.dropped ? ` (+${view.dropped} hidden)` : ""} · ${store.graph.nodes.length} total · built in ${ms}ms${mode}`,
    );
  }

  function applyHighlight(): void {
    const sub = store.mode === "flow" ? store.flow : store.mode === "impact" ? store.impact : null;
    if (!sub) {
      scene.setHighlight(null);
      scene.clearGlows(true);
      return;
    }
    const ids =
      store.mode === "flow"
        ? (store.flow?.nodes ?? [])
        : [...(store.impact?.affected ?? []), store.impact!.target];
    const nodes = new Set<string>();
    for (const id of ids) {
      const r = rep(id);
      if (r) nodes.add(r);
    }
    const links = new Set<string>();
    for (const e of sub.edges) {
      const a = rep(e.from);
      const b = rep(e.to);
      if (a && b && a !== b) links.add(`${a}|${b}`);
    }
    scene.setHighlight({ nodes, links, color: store.mode === "flow" ? FLOW : IMPACT });
    scene.clearGlows(true);
    const focusId = store.mode === "flow" ? store.flow!.root : store.impact!.target;
    const r = rep(focusId);
    if (r) scene.halo(r, store.mode === "flow" ? FLOW : IMPACT);
  }

  function revealAll(ids: Iterable<string>): void {
    const containers = containersToExpand(store.index, ids, store.level);
    let changed = false;
    for (const c of containers) {
      if (!store.expanded.has(c)) {
        store.expanded.add(c);
        changed = true;
      }
    }
    if (changed) store.emit("view", undefined);
  }

  async function startFlow(id: string): Promise<void> {
    stopFlowAnimation();
    try {
      store.flow = await backend.flow(id);
    } catch (err) {
      store.status(`flow failed: ${(err as Error).message}`);
      return;
    }
    store.mode = "flow";
    store.impact = null;
    store.emit("mode", "flow");
    revealAll(store.flow.nodes);
    applyHighlight();
    drawer.showFlow(store.flow);
    animateFlow();
  }

  function animateFlow(): void {
    const flow = store.flow;
    if (!flow) return;
    const lastAtDepth: string[] = [];
    let i = 0;
    for (const step of flow.steps) {
      const parent = step.depth > 0 ? lastAtDepth[step.depth - 1] : undefined;
      lastAtDepth[step.depth] = step.node;
      lastAtDepth.length = step.depth + 1;
      const from = parent ? rep(parent) : null;
      const to = rep(step.node);
      const delay = i * 420;
      flowTimers.push(
        window.setTimeout(() => {
          if (to) scene.flash(to, FLOW, 12, 800);
          if (from && to && from !== to) scene.travel(from, to, FLOW, 400);
        }, delay),
      );
      i++;
    }
    flowTimers.push(
      window.setTimeout(
        () => {
          if (store.mode === "flow") animateFlow();
        },
        i * 420 + 1500,
      ),
    );
  }

  function stopFlowAnimation(): void {
    for (const t of flowTimers) clearTimeout(t);
    flowTimers = [];
  }

  async function startImpact(id: string): Promise<void> {
    stopFlowAnimation();
    try {
      store.impact = await backend.impact(id);
    } catch (err) {
      store.status(`impact failed: ${(err as Error).message}`);
      return;
    }
    store.mode = "impact";
    store.flow = null;
    store.emit("mode", "impact");
    revealAll([...store.impact.affected, id]);
    applyHighlight();
    drawer.showImpact(store.impact);
    const r = rep(id);
    if (r) scene.flash(r, IMPACT, 20, 1200);
  }

  function pickInsight(i: Insight): void {
    store.select(i.node);
    const related = i.kind === "waiting" ? i.on : i.kind === "parallelisable" ? i.children : [];
    const r = rep(i.node);
    if (r) scene.flash(r, IMPACT, 18, 1500);
    for (const id of related) {
      const rr = rep(id);
      if (rr) scene.flash(rr, IMPACT, 10, 1500);
    }
  }

  function onLive(ev: LiveEvent): void {
    if (ev.type === "span") {
      const hops = tracker.record(ev.span);
      if (ev.span.node) {
        const r = rep(ev.span.node);
        if (r) scene.flash(r, ev.span.error ? "#f94144" : "#9ad0ff", 9, 700);
      }
      for (const hop of hops) {
        const a = rep(hop.from);
        const b = rep(hop.to);
        if (a && b && a !== b) scene.travel(a, b, "#9ad0ff", 600, 5);
      }
    } else {
      applyStats(ev.stats);
    }
  }

  function applyStats(stats: NodeStats[]): void {
    for (const s of stats) store.stats.set(s.node, s);
    store.emit("stats", undefined);
    applyHeat();
  }

  function setRuntime(on: boolean): void {
    store.runtimeOn = on;
    store.emit("runtime", undefined);
    if (on) {
      stopStream = backend.stream(onLive, (open) => toolbar.setLive(open));
      backend
        .runtimeStats()
        .then(applyStats)
        .catch(() => undefined);
      store.status("Live: listening on /api/runtime/stream");
    } else {
      stopStream?.();
      stopStream = null;
      toolbar.setLive(false);
    }
  }

  function setHeat(metric: HeatMetric): void {
    store.heat = metric;
    if (metric !== "off" && store.stats.size === 0)
      backend
        .runtimeStats()
        .then(applyStats)
        .catch(() => undefined);
    applyHeat();
  }

  function applyHeat(): void {
    if (store.heat === "off" || store.stats.size === 0) {
      scene.setHeat(null);
      return;
    }
    const metric = (s: NodeStats) =>
      store.heat === "rate"
        ? s.rate_per_sec
        : store.heat === "p95"
          ? s.p95_nanos
          : s.count
            ? s.self_nanos / s.count
            : 0;
    const perRep = new Map<string, number>();
    let max = Number.NEGATIVE_INFINITY;
    let min = Number.POSITIVE_INFINITY;
    for (const s of store.stats.values()) {
      const r = rep(s.node);
      if (!r) continue;
      const v = Math.log1p(metric(s));
      const cur = Math.max(perRep.get(r) ?? Number.NEGATIVE_INFINITY, v);
      perRep.set(r, cur);
      if (cur > max) max = cur;
      if (cur < min) min = cur;
    }
    const span = max - min;
    for (const [k, v] of perRep) perRep.set(k, span > 0 ? 0.15 + (0.85 * (v - min)) / span : 1);
    scene.setHeat(perRep);
  }

  function replayTrace(trace: TraceSummary): void {
    for (const t of replayTimers) clearTimeout(t);
    replayTimers = [];
    let speed = 1;
    const slider = h("input", {
      type: "range",
      min: "0.1",
      max: "4",
      step: "0.1",
      value: "1",
      "aria-label": "Replay speed",
    });
    const label = h("span", { class: "mono small" }, "1.0×");
    const replayBtn = h("button", { class: "ghost small", onclick: () => run() }, "▶ replay");
    slider.addEventListener("input", () => {
      speed = Number(slider.value);
      label.textContent = `${speed.toFixed(1)}×`;
    });
    drawer.showWaterfall(trace, h("span", { class: "replay-controls" }, replayBtn, slider, label));
    revealAll(trace.spans.map((s) => s.node).filter((n): n is string => !!n));
    const run = () => {
      for (const t of replayTimers) clearTimeout(t);
      replayTimers = [];
      const events = scheduleReplay(trace.spans);
      for (const e of events) {
        replayTimers.push(
          window.setTimeout(() => {
            const to = e.span.node ? rep(e.span.node) : null;
            const from = e.from ? rep(e.from) : null;
            if (to) scene.flash(to, e.span.error ? "#f94144" : "#ffe29a", 12, 900);
            if (from && to && from !== to)
              scene.travel(from, to, "#ffe29a", Math.max(250, 600 / speed));
          }, e.at / speed),
        );
      }
    };
    run();
  }

  store.on("view", rebuildView);
  store.on("selection", (id) => {
    const r = id ? rep(id) : null;
    scene.setSelected(r);
    if (r) scene.focus(r);
    if (store.mode === "flow" && store.flow) drawer.showFlow(store.flow);
    else if (store.mode === "impact" && store.impact) drawer.showImpact(store.impact);
  });
  store.on("mode", (mode) => {
    if (mode === "explore") {
      stopFlowAnimation();
      drawer.hide();
      applyHighlight();
    }
  });

  document.addEventListener("keydown", (ev) => {
    const target = ev.target as HTMLElement;
    const typing =
      target.tagName === "INPUT" || target.tagName === "TEXTAREA" || target.tagName === "SELECT";
    if (ev.key === "/" && !typing) {
      ev.preventDefault();
      toolbar.search.focus();
    } else if (ev.key === "Escape") {
      if (typing) target.blur();
      else if (store.mode !== "explore") store.setMode("explore");
      else store.select(null);
    } else if (ev.key === "f" && !typing && store.selected) {
      startFlow(store.selected);
    } else if (ev.key === "i" && !typing && store.selected) {
      startImpact(store.selected);
    }
  });

  setInterval(() => toolbar.setFps(scene.fps()), 1000);
  rebuildView();
  if (isMockMode() && !params.has("synthetic")) setRuntime(true);
  if (params.get("live") === "1") setRuntime(true);
  if (params.has("select")) store.select(params.get("select"));
  if (params.has("flow")) startFlow(params.get("flow")!);
  if (params.has("impact")) startImpact(params.get("impact")!);
  (window as unknown as { hoshizora: unknown }).hoshizora = {
    store,
    scene,
    fps: () => scene.fps(),
    drawCalls: () => scene.drawCalls(),
  };
}

main().catch((err) => {
  document.getElementById("app")!.append(h("div", { class: "error pad" }, String(err)));
});
