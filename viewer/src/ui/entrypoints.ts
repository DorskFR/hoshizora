import { isTestNode } from "../graph/filters";
import type { GraphIndex } from "../graph/query";
import type { EntrypointKind, GraphNode } from "../types";
import { clear, h } from "./dom";

interface Group {
  key: string;
  title: string;
  hint: string;
  kinds: EntrypointKind[];
  optional?: "exports" | "tests";
}

const GROUPS: Group[] = [
  { key: "ui", title: "UI actions", hint: "buttons, forms, event handlers", kinds: ["ui_handler"] },
  {
    key: "http",
    title: "HTTP routes",
    hint: "requests the server answers",
    kinds: ["http_handler"],
  },
  {
    key: "cli",
    title: "Commands & mains",
    hint: "binaries and CLI commands",
    kinds: ["main", "cli"],
  },
  {
    key: "job",
    title: "Jobs & listeners",
    hint: "background work, message handlers",
    kinds: ["job"],
  },
  {
    key: "export",
    title: "Library exports",
    hint: "public API used by other code",
    kinds: ["export"],
    optional: "exports",
  },
  { key: "test", title: "Tests", hint: "test functions", kinds: ["test"], optional: "tests" },
];

const PAGE = 300;

export interface EntryRow {
  node: GraphNode;
  title: string;
  where: string;
  search: string;
}

/** Entrypoints grouped for the picker; tests (by kind or by path) only when asked for. */
export function groupEntrypoints(
  index: GraphIndex,
  opts: { tests: boolean; exports: boolean },
): { group: Group; rows: EntryRow[] }[] {
  const byKind = new Map<EntrypointKind, GraphNode[]>();
  for (const n of index.graph.nodes) {
    if (!n.entrypoint) continue;
    if (!opts.tests && n.entrypoint !== "test" && isTestNode(n)) continue;
    const list = byKind.get(n.entrypoint);
    if (list) list.push(n);
    else byKind.set(n.entrypoint, [n]);
  }
  const out: { group: Group; rows: EntryRow[] }[] = [];
  for (const group of GROUPS) {
    if (group.optional === "tests" && !opts.tests) continue;
    if (group.optional === "exports" && !opts.exports) continue;
    const nodes = group.kinds.flatMap((k) => byKind.get(k) ?? []);
    if (!nodes.length) continue;
    const rows = nodes.map((n) => {
      const routes = (index.in.get(n.id) ?? [])
        .filter((e) => e.kind === "handles")
        .map((e) => index.node(e.from)?.name ?? "");
      const title = routes[0] ?? displayName(n);
      const where = routes.length
        ? `${displayName(n)} · ${shortPath(n.file)}`
        : shortPath(n.file) || n.qualified_name;
      return {
        node: n,
        title,
        where,
        search: `${title} ${where} ${n.qualified_name}`.toLowerCase(),
      };
    });
    rows.sort((a, b) =>
      group.key === "http"
        ? routePath(a.title).localeCompare(routePath(b.title)) || a.title.localeCompare(b.title)
        : (a.node.file ?? "").localeCompare(b.node.file ?? "") ||
          (a.node.span?.start_line ?? 0) - (b.node.span?.start_line ?? 0),
    );
    out.push({ group, rows });
  }
  return out;
}

function routePath(title: string): string {
  return title.replace(/^[A-Z]+\s+/, "");
}

function displayName(n: GraphNode): string {
  const id = n.id.slice(n.id.indexOf("#") + 1);
  return id.includes("::") || id.includes(".") ? id : n.name;
}

function shortPath(file: string | null): string {
  if (!file) return "";
  return file.replace(/^(crates\/|packages\/|apps\/)/, "").replace(/\/src\//, "/");
}

export class EntrypointPicker {
  readonly el: HTMLElement;
  private readonly input: HTMLInputElement;
  private readonly cols: HTMLElement;
  private readonly countEl: HTMLElement;
  private tests = false;
  private exports = false;
  private groups: { group: Group; rows: EntryRow[] }[] = [];
  private showAll = new Set<string>();

  constructor(
    private readonly index: GraphIndex,
    private readonly onStory: (id: string) => void,
    private readonly onInspect: (id: string) => void,
  ) {
    this.input = h("input", {
      type: "search",
      placeholder: "Filter: spawn, sessions, POST /api…",
      "aria-label": "Filter entrypoints",
      oninput: () => this.render(),
      onkeydown: (ev) => {
        if (ev.key === "Enter") {
          const first = this.cols.querySelector<HTMLButtonElement>(".entry-row");
          first?.click();
        }
      },
    });
    const toggle = (label: string, set: (on: boolean) => void) => {
      const box = h("input", {
        type: "checkbox",
        onchange: () => {
          set(box.checked);
          this.rebuild();
        },
      });
      return h("label", { class: "check" }, box, ` ${label}`);
    };
    this.countEl = h("span", { class: "muted small" });
    this.cols = h("div", { class: "entry-cols" });
    this.el = h(
      "div",
      {
        class: "modal-backdrop",
        hidden: true,
        onclick: (ev) => {
          if (ev.target === this.el) this.close();
        },
      },
      h(
        "section",
        { class: "modal entry-picker", role: "dialog", "aria-label": "Flows" },
        h(
          "header",
          { class: "modal-head" },
          h(
            "div",
            null,
            h("h2", null, "Where does it start?"),
            h(
              "p",
              { class: "muted small" },
              "Everything a user, a request or a scheduler can trigger. Pick one to see its story.",
            ),
          ),
          h("span", { class: "spacer" }),
          h(
            "button",
            { class: "ghost small", "aria-label": "Close", onclick: () => this.close() },
            "✕",
          ),
        ),
        h(
          "div",
          { class: "modal-tools" },
          this.input,
          toggle("library exports", (on) => {
            this.exports = on;
          }),
          toggle("tests", (on) => {
            this.tests = on;
          }),
          this.countEl,
        ),
        this.cols,
      ),
    );
  }

  get isOpen(): boolean {
    return !this.el.hidden;
  }

  open(): void {
    this.el.hidden = false;
    if (!this.groups.length) this.rebuild();
    this.input.focus();
    this.input.select();
  }

  close(): void {
    this.el.hidden = true;
  }

  private rebuild(): void {
    this.groups = groupEntrypoints(this.index, { tests: this.tests, exports: this.exports });
    this.render();
  }

  private render(): void {
    clear(this.cols);
    const terms = this.input.value.toLowerCase().split(/\s+/).filter(Boolean);
    let total = 0;
    for (const { group, rows } of this.groups) {
      const matches = terms.length
        ? rows.filter((r) => terms.every((t) => r.search.includes(t)))
        : rows;
      total += matches.length;
      const limit = this.showAll.has(group.key) ? matches.length : PAGE;
      const list = h("div", { class: "entry-list" });
      let lastDir = "";
      for (const r of matches.slice(0, limit)) {
        const dir = group.key === "http" ? "" : dirOf(r.node.file);
        if (dir && dir !== lastDir) {
          list.append(h("div", { class: "entry-dir muted mono small" }, dir));
          lastDir = dir;
        }
        list.append(
          h(
            "div",
            { class: "entry-item" },
            h(
              "button",
              {
                class: "entry-row",
                title: `Open the story of ${r.node.qualified_name}`,
                onclick: () => this.pickStory(r.node.id),
              },
              h("span", { class: group.key === "http" ? "mono" : "" }, r.title),
              h(
                "span",
                { class: "muted small mono" },
                group.key === "http" ? r.where : baseOf(r.node.file),
              ),
            ),
            h(
              "button",
              {
                class: "ghost small inspect",
                title: "Inspect without opening the story",
                onclick: () => {
                  this.close();
                  this.onInspect(r.node.id);
                },
              },
              "ⓘ",
            ),
          ),
        );
      }
      if (matches.length > limit) {
        list.append(
          h(
            "button",
            {
              class: "link small pad",
              onclick: () => {
                this.showAll.add(group.key);
                this.render();
              },
            },
            `Show all ${matches.length}`,
          ),
        );
      }
      if (!matches.length) list.append(h("p", { class: "muted small pad" }, "No match."));
      this.cols.append(
        h(
          "section",
          { class: `entry-col entry-${group.key}` },
          h("h3", null, `${group.title} `, h("span", { class: "count" }, String(matches.length))),
          h("p", { class: "muted small hint" }, group.hint),
          list,
        ),
      );
    }
    this.countEl.textContent = `${total} entrypoints${this.tests ? "" : " · tests hidden"}`;
  }

  private pickStory(id: string): void {
    this.close();
    this.onStory(id);
  }
}

function dirOf(file: string | null): string {
  if (!file) return "";
  const parts = shortPath(file).split("/");
  parts.pop();
  return parts.join("/");
}

function baseOf(file: string | null): string {
  return file?.split("/").pop() ?? "";
}
