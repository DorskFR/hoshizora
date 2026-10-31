import "./styles.css";
import { type Backend, createProjects, type ProjectsApi } from "./api";
import { FLOW, IMPACT } from "./graph/colors";
import { buildView, containersToExpand } from "./graph/lod";
import { GraphIndex } from "./graph/query";
import { Scene } from "./graph/scene";
import { formatRoute, parseRoute, type Route } from "./router";
import { SpanTracker, scheduleReplay } from "./runtime/tracker";
import { type HeatMetric, Store } from "./state";
import { storyFromFlow } from "./story/build";
import type {
  Edge,
  Insight,
  LiveEvent,
  NodeStats,
  Project,
  RuntimeSpan,
  Story,
  TraceSummary,
} from "./types";
import { fmtMs, h, shortId } from "./ui/dom";
import { Drawer } from "./ui/drawer";
import { EntrypointPicker } from "./ui/entrypoints";
import { ProjectHome } from "./ui/home";
import { Legend } from "./ui/legend";
import { InsightsPanel, TracesPanel } from "./ui/lists";
import { NodePanel } from "./ui/panel";
import { panelResizer } from "./ui/resize";
import { StoryView } from "./ui/story";
import { Toolbar } from "./ui/toolbar";

interface Mounted {
  applyRoute(r: Route): void;
}

function splashEl(...lines: (Node | string)[]): HTMLElement {
  return h("div", { class: "splash" }, h("span", { class: "kanji" }, "星空"), ...lines);
}

async function boot(): Promise<void> {
  const app = document.getElementById("app")!;
  const projects = await createProjects();
  let list: Project[] | null = null;
  try {
    list = await projects.list();
  } catch (err) {
    console.warn("project list unavailable", err);
  }
  const multi = list !== null;
  const normalise = (r: Route): Route =>
    !multi && r.view === "home" ? { ...r, project: null, view: "map" } : r;
  const isHome = (r: Route) => multi && r.project === null && r.view === "home";

  const initial = normalise(parseRoute(location.hash));
  const shown = { home: isHome(initial), project: initial.project };
  let mounted: Mounted | null = null;
  window.addEventListener("hashchange", () => {
    const r = normalise(parseRoute(location.hash));
    if (isHome(r) !== shown.home || (!shown.home && r.project !== shown.project)) {
      location.reload();
      return;
    }
    mounted?.applyRoute(r);
  });

  const open = async (route: Route) => {
    let project: Project | null = null;
    if (route.project !== null) {
      project = await waitUntilReady(app, projects, route.project);
      if (!project) return;
    }
    mounted = await mountProject(app, projects.backend(route.project), { project, multi, route });
  };

  if (!shown.home) {
    await open(initial);
    return;
  }
  const view = new ProjectHome(projects, (id) => {
    const route: Route = { project: id, view: "map", node: null };
    view.stop();
    view.el.remove();
    shown.home = false;
    shown.project = id;
    location.hash = formatRoute(route);
    void open(route);
  });
  app.append(view.el);
  await view.start();
}

async function waitUntilReady(
  app: HTMLElement,
  projects: ProjectsApi,
  id: string,
): Promise<Project | null> {
  const status = h("span", null, "loading project…");
  const splash = splashEl(status);
  app.append(splash);
  for (;;) {
    let p: Project;
    try {
      p = await projects.get(id);
    } catch (err) {
      splash.replaceChildren(
        h("span", { class: "kanji" }, "星空"),
        h("span", { class: "error" }, `Unknown project ${id}: ${(err as Error).message}`),
        h("a", { href: "#/", class: "button" }, "All projects"),
      );
      return null;
    }
    if (p.status === "ready") {
      splash.remove();
      return p;
    }
    if (p.status === "failed") {
      splash.replaceChildren(
        h("span", { class: "kanji" }, "星空"),
        h("span", { class: "error" }, `${p.name} failed to index: ${p.error ?? "unknown error"}`),
        h("a", { href: "#/", class: "button" }, "All projects"),
      );
      return null;
    }
    status.textContent = `${p.name}: ${p.status}…`;
    await new Promise((r) => setTimeout(r, 1500));
  }
}

async function mountProject(
  app: HTMLElement,
  backend: Backend,
  ctx: { project: Project | null; multi: boolean; route: Route },
): Promise<Mounted> {
  const store = new Store(backend, ctx.project?.name ?? null);
  const projectId = ctx.route.project;
  const canvasHost = h("div", { class: "canvas" });
  const splash = splashEl(h("span", null, "loading graph…"));
  app.append(canvasHost, splash);

  try {
    store.graph = await backend.graph();
  } catch (err) {
    splash.replaceChildren(
      h("span", { class: "kanji" }, "星空"),
      h("span", { class: "error" }, `Cannot load the graph: ${(err as Error).message}`),
      h("span", { class: "muted" }, "Run `hoshizora serve`, or open with ?mock for fixtures."),
    );
    return { applyRoute: () => undefined };
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

  const ticker = h("ul", { class: "ticker", "aria-label": "Recent spans", "aria-live": "off" });
  app.append(ticker);
  function tick(span: RuntimeSpan): void {
    const dur = (span.end_unix_nanos - span.start_unix_nanos) / 1e6;
    const node = span.node ? (store.index.node(span.node)?.name ?? null) : null;
    const li = h(
      "li",
      { class: span.error ? "tick err" : "tick" },
      h("b", null, span.service),
      ` · ${node ?? span.name}`,
      h("span", { class: "muted" }, ` ${fmtMs(dur)}`),
    );
    ticker.prepend(li);
    while (ticker.childElementCount > 10) ticker.lastElementChild?.remove();
  }

  const go = (r: Omit<Route, "project">) => {
    location.hash = formatRoute({ ...r, project: projectId });
  };
  const openStory = (id: string) => go({ view: "story", node: id });
  const drawer = new Drawer(store);
  const picker = new EntrypointPicker(store.index, openStory, (id) => store.select(id));
  const panel = new NodePanel(store, {
    story: openStory,
    impact: (id) => startImpact(id),
    browseFlows: () => picker.open(),
  });
  const story = new StoryView(store.index, {
    select: (id) => store.select(id),
    close: () => go({ view: "map", node: store.selected }),
    showOnMap: (s) => showStoryOnMap(s),
  });
  const insights = new InsightsPanel(store, (i) => pickInsight(i));
  const traces = new TracesPanel(store, (t) => replayTrace(t));
  const legend = new Legend(store, (mode) => scene.setColorBy(mode));
  const toolbar = new Toolbar(store, {
    runtime: (on) => setRuntime(on),
    heat: (metric) => setHeat(metric),
    togglePanel: (name) => (name === "insights" ? insights.toggle() : traces.toggle()),
    zoomToFit: () => scene.zoomToFit(),
    story: openStory,
    browseFlows: () => picker.open(),
    home: ctx.multi
      ? () => {
          location.hash = "#/";
        }
      : null,
  });
  app.append(
    toolbar.el,
    story.el,
    h("div", { class: "right" }, panelResizer(app), panel.el, insights.el, traces.el),
    legend.el,
    drawer.el,
    picker.el,
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
      hideKinds: store.hideKinds,
      hideTests: store.hideTests,
      foldRoutes: store.foldRoutes,
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

  let storyToken = 0;
  async function loadStory(id: string): Promise<void> {
    const token = ++storyToken;
    const name = store.index.node(id)?.name ?? shortId(id);
    document.body.classList.add("story-open");
    scene.pause(true);
    story.showLoading(name);
    let result: Story | null = null;
    let approximate = false;
    try {
      result = await backend.story(id);
      if (!result || result.steps.length === 0) {
        approximate = true;
        result = storyFromFlow(store.index, await backend.flow(id, 12));
      }
    } catch (err) {
      if (token === storyToken)
        story.showError(name, `Could not build the story: ${(err as Error).message}`);
      return;
    }
    if (token !== storyToken) return;
    story.show(result, { approximate });
    story.setSelected(id);
    if (store.selected !== id) store.select(id);
  }

  function closeStory(): void {
    storyToken++;
    if (!story.open) return;
    story.hide();
    document.body.classList.remove("story-open");
    scene.pause(false);
  }

  function showStoryOnMap(s: Story): void {
    const nodes = new Set<string>();
    for (const step of s.steps) {
      nodes.add(step.node);
      for (const f of step.folded) nodes.add(f);
    }
    const edges: Edge[] = s.links.map((l) => ({
      from: l.from,
      to: l.to,
      kind: l.kind,
      confidence: l.confidence,
      line: null,
      loop_depth: 0,
      conditional: false,
      awaited: !l.detached,
      attrs: {},
    }));
    store.flow = { root: s.root, nodes: [...nodes], edges, steps: [], truncated: s.truncated };
    store.impact = null;
    store.mode = "flow";
    go({ view: "map", node: s.root });
    store.emit("mode", "flow");
    revealAll(nodes);
    applyHighlight();
  }

  async function startImpact(id: string): Promise<void> {
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
      tick(ev.span);
      const hops = tracker.record(ev.span);
      if (ev.span.node) {
        const r = rep(ev.span.node);
        if (r) scene.flash(r, ev.span.error ? "#f94144" : "#7fd0ff", 16, 1500);
      }
      for (const hop of hops) {
        const a = rep(hop.from);
        const b = rep(hop.to);
        if (a && b && a !== b) scene.travel(a, b, "#7fd0ff", 900, 7);
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
    ticker.classList.toggle("on", on);
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
    if (story.open) {
      story.setSelected(id);
      return;
    }
    const r = id ? rep(id) : null;
    scene.setSelected(r);
    if (r) scene.focus(r);
    if (store.mode === "impact" && store.impact) drawer.showImpact(store.impact);
    const want = formatRoute({ project: projectId, view: "map", node: id });
    if (location.hash !== want) history.replaceState(null, "", want);
  });
  store.on("mode", (mode) => {
    if (mode === "explore") {
      drawer.hide();
      applyHighlight();
    }
  });

  document.addEventListener("keydown", (ev) => {
    const target = ev.target as HTMLElement;
    const typing =
      target.tagName === "INPUT" || target.tagName === "TEXTAREA" || target.tagName === "SELECT";
    if (ev.key === "Escape") {
      if (typing) target.blur();
      else if (picker.isOpen) picker.close();
      else if (story.open) go({ view: "map", node: store.selected });
      else if (store.mode !== "explore") store.setMode("explore");
      else store.select(null);
      return;
    }
    if (typing || ev.ctrlKey || ev.metaKey || ev.altKey) return;
    if (ev.key === "/") {
      ev.preventDefault();
      toolbar.search.focus();
    } else if (ev.key === "e") {
      ev.preventDefault();
      picker.open();
    } else if (ev.key === "s" && store.selected) {
      ev.preventDefault();
      openStory(store.selected);
    } else if (ev.key === "i" && store.selected && !story.open) {
      startImpact(store.selected);
    } else if (story.open && (ev.key === "+" || ev.key === "=")) {
      story.zoomBy(1.25);
    } else if (story.open && ev.key === "-") {
      story.zoomBy(0.8);
    } else if (story.open && ev.key === "0") {
      story.fit("all");
    }
  });

  function applyRoute(r: Route): void {
    if (r.view === "story" && r.node) {
      if (!story.open || story.root !== r.node) void loadStory(r.node);
      return;
    }
    closeStory();
    if (r.node && r.node !== store.selected) store.select(r.node);
  }

  setInterval(() => toolbar.setFps(scene.fps()), 1000);
  rebuildView();
  if (params.get("live") === "1") setRuntime(true);
  if (params.has("select")) store.select(params.get("select"));
  if (params.has("flow")) openStory(params.get("flow")!);
  if (params.has("impact")) startImpact(params.get("impact")!);
  applyRoute(ctx.route);
  (window as unknown as { hoshizora: unknown }).hoshizora = {
    store,
    scene,
    fps: () => scene.fps(),
    drawCalls: () => scene.drawCalls(),
  };
  return { applyRoute };
}

boot().catch((err) => {
  document.getElementById("app")!.append(h("div", { class: "error pad" }, String(err)));
});
