import type { Level } from "../graph/lod";
import type { HeatMetric, Store } from "../state";
import type { EntrypointKind, GraphNode, SearchHit } from "../types";
import { clear, h } from "./dom";

export interface ToolbarActions {
  runtime(on: boolean): void;
  heat(metric: HeatMetric): void;
  togglePanel(name: "insights" | "traces"): void;
  zoomToFit(): void;
}

const ENTRY_GROUPS: { kind: EntrypointKind; label: string }[] = [
  { kind: "ui_handler", label: "UI handlers" },
  { kind: "http_handler", label: "HTTP handlers" },
  { kind: "main", label: "main" },
  { kind: "cli", label: "CLI" },
  { kind: "job", label: "Jobs" },
  { kind: "test", label: "Tests" },
  { kind: "export", label: "Exports" },
];

export class Toolbar {
  readonly el: HTMLElement;
  readonly search: HTMLInputElement;
  private readonly results: HTMLElement;
  private readonly entryMenu: HTMLElement;
  private readonly fpsEl: HTMLElement;
  private readonly statusEl: HTMLElement;
  private readonly modeEl: HTMLElement;
  private readonly runtimeBtn: HTMLButtonElement;
  private readonly liveDot: HTMLElement;
  private debounce = 0;
  private active = -1;
  private hits: SearchHit[] = [];

  constructor(
    private readonly store: Store,
    private readonly actions: ToolbarActions,
  ) {
    this.search = h("input", {
      type: "search",
      placeholder: "Search symbols, files, routes, tables…  ( / )",
      "aria-label": "Search nodes",
      autocomplete: "off",
      oninput: () => this.onInput(),
      onkeydown: (ev) => this.onKey(ev),
    });
    this.results = h("ul", { class: "results", role: "listbox", hidden: true });
    this.entryMenu = h("div", { class: "menu", hidden: true, role: "menu" });
    const entryBtn = h(
      "button",
      { class: "ghost", "aria-label": "Entrypoints", onclick: () => this.toggleEntrypoints() },
      "⚑ Entrypoints",
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

    this.el = h(
      "header",
      { class: "toolbar" },
      h(
        "div",
        { class: "brand" },
        h("span", { class: "kanji" }, "星空"),
        h("span", null, "hoshizora"),
      ),
      h("div", { class: "search-wrap" }, this.search, this.results),
      h("div", { class: "entry-wrap" }, entryBtn, this.entryMenu),
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
      if (!this.el.contains(ev.target as Node)) {
        this.results.hidden = true;
        this.entryMenu.hidden = true;
      }
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
        this.store.mode === "flow" ? "Flow" : "Impact",
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
    this.debounce = window.setTimeout(async () => {
      try {
        this.hits = await this.store.backend.search(q, 15);
      } catch {
        this.hits = [];
      }
      this.active = -1;
      this.renderResults();
    }, 120);
  }

  private renderResults(): void {
    clear(this.results);
    this.results.hidden = this.hits.length === 0;
    this.hits.forEach((hit, i) => {
      this.results.append(
        h(
          "li",
          {
            role: "option",
            class: i === this.active ? "active" : "",
            onclick: () => this.pick(hit.node),
          },
          h("span", { class: `kind kind-${hit.kind}` }, hit.kind),
          h("span", null, hit.name),
          h("span", { class: "muted mono small" }, hit.qualified_name),
        ),
      );
    });
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
      if (hit) this.pick(hit.node);
    } else if (ev.key === "Escape") {
      this.results.hidden = true;
      this.search.blur();
    }
  }

  private pick(id: string): void {
    this.results.hidden = true;
    this.entryMenu.hidden = true;
    this.search.value = "";
    this.store.select(id);
  }

  private async toggleEntrypoints(): Promise<void> {
    if (!this.entryMenu.hidden) {
      this.entryMenu.hidden = true;
      return;
    }
    clear(this.entryMenu);
    this.entryMenu.hidden = false;
    this.entryMenu.append(h("div", { class: "muted pad" }, "Loading…"));
    let entries: GraphNode[] = [];
    try {
      entries = await this.store.backend.entrypoints();
    } catch (err) {
      clear(this.entryMenu);
      this.entryMenu.append(h("div", { class: "error pad" }, (err as Error).message));
      return;
    }
    clear(this.entryMenu);
    for (const group of ENTRY_GROUPS) {
      const items = entries.filter((e) => e.entrypoint === group.kind);
      if (!items.length) continue;
      this.entryMenu.append(h("div", { class: "menu-group" }, `${group.label} (${items.length})`));
      for (const n of items.slice(0, 50)) {
        this.entryMenu.append(
          h(
            "button",
            { class: "menu-item", role: "menuitem", onclick: () => this.pick(n.id) },
            h("span", null, n.name),
            h("span", { class: "muted mono small" }, n.file ?? n.qualified_name),
          ),
        );
      }
    }
    if (!this.entryMenu.childElementCount)
      this.entryMenu.append(h("div", { class: "muted pad" }, "No entrypoints"));
  }
}
