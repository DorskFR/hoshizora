import { isTestNode } from "../graph/filters";
import type { Level } from "../graph/lod";
import type { HeatMetric, Store } from "../state";
import type { SearchHit } from "../types";
import { clear, h } from "./dom";

export interface ToolbarActions {
  runtime(on: boolean): void;
  heat(metric: HeatMetric): void;
  togglePanel(name: "insights" | "traces"): void;
  zoomToFit(): void;
  story(id: string): void;
  browseFlows(): void;
  home: (() => void) | null;
}

const STORY_KINDS = new Set(["function", "route", "type"]);
const SHOWN_HITS = 14;

export class Toolbar {
  readonly el: HTMLElement;
  readonly search: HTMLInputElement;
  private readonly results: HTMLElement;
  private readonly fpsEl: HTMLElement;
  private readonly statusEl: HTMLElement;
  private readonly modeEl: HTMLElement;
  private readonly runtimeBtn: HTMLButtonElement;
  private readonly liveDot: HTMLElement;
  private debounce = 0;
  private active = -1;
  private hits: SearchHit[] = [];
  private hiddenTests = 0;
  private showTests = false;

  constructor(
    private readonly store: Store,
    private readonly actions: ToolbarActions,
  ) {
    this.search = h("input", {
      type: "search",
      placeholder: "Search functions, routes, tables…  ( / )",
      "aria-label": "Search nodes",
      autocomplete: "off",
      oninput: () => this.onInput(),
      onkeydown: (ev) => this.onKey(ev),
    });
    this.results = h("ul", { class: "results", role: "listbox", hidden: true });
    const flowsBtn = h(
      "button",
      {
        class: "primary",
        title: "Every UI action, HTTP route, command and job: pick one to see its story (e)",
        onclick: () => this.actions.browseFlows(),
      },
      "▶ Flows",
    );

    const levelSel = h(
      "select",
      {
        "aria-label": "Level of detail",
        onchange: () => this.store.setLevel(levelSel.value as Level),
      },
      h("option", { value: "packages" }, "Packages"),
      h("option", { value: "files" }, "Files"),
      h("option", { value: "functions" }, "Functions"),
    );
    levelSel.value = store.level;
    store.on("view", () => {
      levelSel.value = store.level;
    });

    const heatSel = h(
      "select",
      {
        "aria-label": "Heat overlay",
        onchange: () => this.actions.heat(heatSel.value as HeatMetric),
      },
      h("option", { value: "off" }, "Heat: off"),
      h("option", { value: "self" }, "Heat: self time"),
      h("option", { value: "rate" }, "Heat: call rate"),
      h("option", { value: "p95" }, "Heat: p95"),
    );
    this.liveDot = h("span", { class: "dot" });
    this.runtimeBtn = h(
      "button",
      {
        class: "ghost",
        "aria-pressed": "false",
        onclick: () => this.actions.runtime(!this.store.runtimeOn),
      },
      this.liveDot,
      " Live",
    );
    this.fpsEl = h("span", { class: "fps mono", "aria-label": "Frames per second" }, "– fps");
    this.statusEl = h("span", { class: "status muted", "aria-live": "polite" });
    this.modeEl = h("span", { class: "mode" });

    const tools = h(
      "div",
      { class: "toolbar-tools" },
      levelSel,
      this.modeEl,
      h("span", { class: "spacer" }),
      this.statusEl,
      this.runtimeBtn,
      heatSel,
      h(
        "button",
        { class: "ghost", onclick: () => this.actions.togglePanel("insights") },
        "Insights",
      ),
      h("button", { class: "ghost", onclick: () => this.actions.togglePanel("traces") }, "Traces"),
      h(
        "button",
        {
          class: "ghost",
          "aria-label": "Fit graph to view",
          onclick: () => this.actions.zoomToFit(),
        },
        "⤢",
      ),
      this.fpsEl,
    );

    this.el = h(
      "header",
      { class: "toolbar" },
      h(
        actions.home ? "button" : "div",
        {
          class: "brand",
          title: actions.home ? "All projects" : "",
          onclick: () => actions.home?.(),
        },
        h("span", { class: "kanji" }, "星空"),
        h("span", null, "hoshizora"),
      ),
      store.projectName && h("span", { class: "project-name" }, store.projectName),
      flowsBtn,
      h("div", { class: "search-wrap" }, this.search, this.results),
      tools,
    );

    store.on("status", (msg) => {
      this.statusEl.textContent = msg;
    });
    store.on("mode", () => this.renderMode());
    store.on("runtime", () => {
      this.runtimeBtn.setAttribute("aria-pressed", String(this.store.runtimeOn));
      this.runtimeBtn.classList.toggle("on", this.store.runtimeOn);
    });
    this.renderMode();
    document.addEventListener("click", (ev) => {
      if (!this.el.contains(ev.target as Node)) this.results.hidden = true;
    });
  }

  setFps(fps: number): void {
    this.fpsEl.textContent = `${fps} fps`;
  }

  setLive(open: boolean): void {
    this.liveDot.classList.toggle("live", open);
  }

  private renderMode(): void {
    clear(this.modeEl);
    if (this.store.mode === "explore") return;
    this.modeEl.append(
      h(
        "span",
        { class: `chip mode-${this.store.mode}` },
        this.store.mode === "flow" ? "Story on map" : "Impact",
      ),
      h(
        "button",
        {
          class: "ghost small",
          "aria-label": "Exit mode",
          onclick: () => this.store.setMode("explore"),
        },
        "✕",
      ),
    );
  }

  private onInput(): void {
    window.clearTimeout(this.debounce);
    const q = this.search.value.trim();
    if (!q) {
      this.results.hidden = true;
      return;
    }
    this.debounce = window.setTimeout(() => this.runSearch(q), 120);
  }

  private async runSearch(q: string): Promise<void> {
    let all: SearchHit[] = [];
    try {
      all = await this.store.backend.search(q, 80);
    } catch {
      all = [];
    }
    if (this.search.value.trim() !== q) return;
    const isTest = (hit: SearchHit) => {
      const n = this.store.index.node(hit.node);
      return n ? isTestNode(n) : false;
    };
    const kept = this.showTests ? all : all.filter((hit) => !isTest(hit));
    this.hiddenTests = all.length - kept.length;
    this.hits = kept.slice(0, SHOWN_HITS);
    this.active = -1;
    this.renderResults();
  }

  private renderResults(): void {
    clear(this.results);
    this.results.hidden = false;
    if (!this.hits.length)
      this.results.append(h("li", { class: "muted pad" }, "No matches outside tests."));
    this.hits.forEach((hit, i) => {
      const n = this.store.index.node(hit.node);
      const where = n?.file
        ? `${n.file}${n.span ? `:${n.span.start_line}` : ""}`
        : hit.qualified_name;
      const canStory = STORY_KINDS.has(hit.kind);
      this.results.append(
        h(
          "li",
          {
            role: "option",
            class: i === this.active ? "active" : "",
            onclick: () => this.pick(hit.node),
          },
          h(
            "span",
            { class: `kind kind-${hit.kind}` },
            n?.entrypoint === "test" ? "test" : hit.kind,
          ),
          h(
            "span",
            { class: "hit-main" },
            h("b", null, hit.name),
            n?.entrypoint && n.entrypoint !== "test"
              ? h("span", { class: "chip entry small" }, n.entrypoint.replace("_", " "))
              : null,
            h("span", { class: "muted mono small hit-file" }, where),
          ),
          canStory &&
            h(
              "button",
              {
                class: "ghost small story-btn",
                title: "Open the story from here",
                onclick: (ev) => {
                  ev.stopPropagation();
                  this.results.hidden = true;
                  this.search.value = "";
                  this.actions.story(hit.node);
                },
              },
              "▶ Story",
            ),
        ),
      );
    });
    if (this.hiddenTests || this.showTests) {
      const toggle = h("input", {
        type: "checkbox",
        checked: this.showTests,
        onchange: () => {
          this.showTests = toggle.checked;
          void this.runSearch(this.search.value.trim());
        },
      });
      this.results.append(
        h(
          "li",
          { class: "results-foot muted small" },
          h(
            "label",
            { class: "check" },
            toggle,
            this.showTests ? " include tests" : ` include tests (${this.hiddenTests} hidden)`,
          ),
        ),
      );
    }
  }

  private onKey(ev: KeyboardEvent): void {
    if (ev.key === "ArrowDown" || ev.key === "ArrowUp") {
      ev.preventDefault();
      if (!this.hits.length) return;
      this.active =
        (this.active + (ev.key === "ArrowDown" ? 1 : -1) + this.hits.length) % this.hits.length;
      this.renderResults();
    } else if (ev.key === "Enter") {
      const hit = this.hits[this.active] ?? this.hits[0];
      if (!hit) return;
      if (ev.shiftKey && STORY_KINDS.has(hit.kind)) {
        this.results.hidden = true;
        this.search.value = "";
        this.actions.story(hit.node);
      } else this.pick(hit.node);
    } else if (ev.key === "Escape") {
      this.results.hidden = true;
      this.search.blur();
    }
  }

  private pick(id: string): void {
    this.results.hidden = true;
    this.search.value = "";
    this.store.select(id);
  }
}
