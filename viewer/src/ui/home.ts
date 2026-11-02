import type { ProjectsApi } from "../api";
import type { Project, ProjectStorage, Storage } from "../types";
import { clear, h } from "./dom";

const POLL_MS = 1500;
const STATUS_TEXT: Record<Project["status"], string> = {
  cloning: "Cloning…",
  indexing: "Indexing…",
  ready: "Ready",
  failed: "Failed",
};

export function sourceText(p: Project): string {
  if (p.source.kind === "path") return p.source.path;
  return p.source.ref ? `${p.source.url} @ ${p.source.ref}` : p.source.url;
}

function ago(iso: string | null): string {
  if (!iso) return "";
  const s = (Date.now() - Date.parse(iso)) / 1000;
  if (Number.isNaN(s)) return iso;
  if (s < 60) return "just now";
  if (s < 3600) return `${Math.round(s / 60)} min ago`;
  if (s < 86400) return `${Math.round(s / 3600)} h ago`;
  return `${Math.round(s / 86400)} d ago`;
}

function bytes(n: number): string {
  if (n < 1024) return `${n} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let v = n / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v < 10 ? v.toFixed(1) : Math.round(v)} ${units[i]}`;
}

function compact(n: number): string {
  return n >= 10_000
    ? `${Math.round(n / 1000)}k`
    : n >= 1000
      ? `${(n / 1000).toFixed(1)}k`
      : String(n);
}

/** Landing screen: registered projects, their indexing state, and a field to add a repository. */
export class ProjectHome {
  readonly el: HTMLElement;
  private readonly list: HTMLElement;
  private readonly usage: HTMLElement;
  private readonly purgeAllBtn: HTMLButtonElement;
  private readonly error: HTMLElement;
  private readonly url: HTMLInputElement;
  private readonly ref: HTMLInputElement;
  private readonly addBtn: HTMLButtonElement;
  private projects: Project[] = [];
  private storage: Storage | null = null;
  private timer = 0;
  private openWhenReady: string | null = null;

  constructor(
    private readonly api: ProjectsApi,
    private readonly onOpen: (id: string) => void,
  ) {
    this.url = h("input", {
      type: "text",
      placeholder: "https://github.com/owner/repo",
      "aria-label": "Git URL",
      autocomplete: "off",
      spellcheck: false,
    });
    this.ref = h("input", {
      type: "text",
      placeholder: "branch / tag (optional)",
      "aria-label": "Git ref",
      autocomplete: "off",
      spellcheck: false,
      class: "ref",
    });
    this.addBtn = h("button", { class: "primary", type: "submit" }, "Add repository");
    this.error = h("p", { class: "error small", hidden: true });
    this.list = h("div", { class: "project-list" });
    this.usage = h("span", { class: "small muted", hidden: true });
    this.purgeAllBtn = h(
      "button",
      {
        hidden: true,
        title: "Delete every idle clone to free disk space; graphs stay and reindex clones again",
        onclick: () => {
          if (
            confirm(
              "Purge all clones? Projects and their graphs stay browsable; source is unavailable until each is reindexed.",
            )
          )
            void this.act(() => this.api.purgeAll());
        },
      },
      "Purge all clones",
    );
    const form = h("form", { class: "add-form" }, this.url, this.ref, this.addBtn);
    form.addEventListener("submit", (ev) => {
      ev.preventDefault();
      void this.add();
    });
    this.el = h(
      "main",
      { class: "home" },
      h(
        "header",
        { class: "home-head" },
        h("span", { class: "kanji" }, "星空"),
        h(
          "div",
          null,
          h("h1", null, "hoshizora"),
          h("p", { class: "muted" }, "Pick a codebase to explore, or add one."),
        ),
      ),
      h("section", { class: "home-add" }, h("h3", null, "Add a repository"), form, this.error),
      h(
        "section",
        null,
        h(
          "div",
          { class: "row projects-head" },
          h("h3", null, "Projects"),
          this.usage,
          this.purgeAllBtn,
        ),
        this.list,
      ),
    );
  }

  async start(): Promise<void> {
    await this.refresh();
    this.url.focus();
  }

  stop(): void {
    window.clearTimeout(this.timer);
  }

  private async refresh(): Promise<void> {
    window.clearTimeout(this.timer);
    try {
      [this.projects, this.storage] = await Promise.all([
        this.api.list().then((p) => p ?? []),
        this.api.storage().catch(() => null),
      ]);
    } catch (err) {
      this.showError(`Cannot list projects: ${(err as Error).message}`);
    }
    this.render();
    const pending = this.projects.some((p) => p.status === "cloning" || p.status === "indexing");
    if (this.openWhenReady) {
      const p = this.projects.find((x) => x.id === this.openWhenReady);
      if (p?.status === "ready") {
        this.openWhenReady = null;
        this.onOpen(p.id);
        return;
      }
      if (p?.status === "failed") this.openWhenReady = null;
    }
    if (pending) this.timer = window.setTimeout(() => this.refresh(), POLL_MS);
  }

  private showError(msg: string | null): void {
    this.error.hidden = !msg;
    this.error.textContent = msg ?? "";
  }

  private async add(): Promise<void> {
    const url = this.url.value.trim();
    if (!url) {
      this.url.focus();
      return;
    }
    this.showError(null);
    this.addBtn.disabled = true;
    try {
      const p = await this.api.add({ url, ref: this.ref.value.trim() || null });
      this.url.value = "";
      this.ref.value = "";
      this.openWhenReady = p.id;
      await this.refresh();
    } catch (err) {
      this.showError(`Could not add ${url}: ${(err as Error).message}`);
    } finally {
      this.addBtn.disabled = false;
    }
  }

  private async act(fn: () => Promise<unknown>): Promise<void> {
    try {
      await fn();
    } catch (err) {
      this.showError((err as Error).message);
    }
    await this.refresh();
  }

  private storageOf(id: string): ProjectStorage | undefined {
    return this.storage?.projects.find((s) => s.id === id);
  }

  private renderUsage(): void {
    const s = this.storage;
    this.usage.hidden = !s;
    this.purgeAllBtn.hidden = !s;
    if (!s) return;
    const parts = [`clones ${bytes(s.clone_bytes)}`, `graphs ${bytes(s.graph_bytes)}`];
    if (s.disk) parts.push(`${bytes(s.disk.free_bytes)} free of ${bytes(s.disk.total_bytes)}`);
    this.usage.textContent = parts.join(" · ");
    this.purgeAllBtn.disabled = !s.projects.some((p) => p.clone_bytes !== null);
  }

  private render(): void {
    clear(this.list);
    this.renderUsage();
    if (!this.projects.length) {
      this.list.append(
        h("p", { class: "muted pad" }, "No projects yet. Paste a git URL above to index one."),
      );
      return;
    }
    for (const p of this.projects) {
      const ready = p.status === "ready";
      const busy = p.status === "cloning" || p.status === "indexing";
      const usage = this.storageOf(p.id);
      const hasClone = usage?.clone_bytes != null;
      const card = h(
        "article",
        { class: `project-card status-${p.status}` },
        h(
          "div",
          { class: "project-main" },
          h(
            "div",
            { class: "row" },
            h("h2", null, p.name),
            h(
              "span",
              { class: `status-badge status-${p.status}` },
              busy && h("i", { class: "spinner" }),
              STATUS_TEXT[p.status],
            ),
            this.openWhenReady === p.id &&
              busy &&
              h("span", { class: "muted small" }, "opens when ready"),
          ),
          h("div", { class: "mono small muted wrap" }, sourceText(p)),
          h(
            "div",
            { class: "small muted project-meta" },
            p.commit && h("span", { class: "mono" }, p.commit.slice(0, 8)),
            p.indexed_at && h("span", null, `indexed ${ago(p.indexed_at)}`),
            ready && h("span", null, `${compact(p.nodes)} nodes · ${compact(p.edges)} edges`),
            usage &&
              h(
                "span",
                { title: "Disk used by this project's clone and graph" },
                p.source.kind === "git"
                  ? `clone ${hasClone ? bytes(usage.clone_bytes ?? 0) : "purged"} · graph ${bytes(usage.graph_bytes)}`
                  : `graph ${bytes(usage.graph_bytes)}`,
              ),
          ),
          p.source.kind === "git" &&
            usage &&
            !hasClone &&
            !busy &&
            h(
              "div",
              { class: "small muted" },
              "Clone purged: source is not available until the project is reindexed.",
            ),
          p.error && h("div", { class: "error small" }, p.error),
        ),
        h(
          "div",
          { class: "project-actions" },
          h(
            "button",
            { class: "primary", disabled: !ready, onclick: () => this.onOpen(p.id) },
            "Open",
          ),
          h(
            "button",
            {
              disabled: busy,
              title: "Fetch the latest commit and rebuild the graph",
              onclick: () => this.act(() => this.api.reindex(p.id)),
            },
            "Reindex",
          ),
          p.source.kind === "git" &&
            h(
              "button",
              {
                disabled: busy || !hasClone,
                title:
                  "Delete the clone to free disk space; the graph stays and reindex clones again",
                onclick: () => {
                  if (
                    confirm(
                      `Purge the clone of ${p.name}? The graph stays browsable; source is unavailable until it is reindexed.`,
                    )
                  )
                    void this.act(() => this.api.purge(p.id));
                },
              },
              "Purge clone",
            ),
          h(
            "button",
            {
              class: "danger",
              disabled: busy,
              onclick: () => {
                if (
                  confirm(`Remove ${p.name} from hoshizora? The clone and its graph are deleted.`)
                )
                  void this.act(() => this.api.remove(p.id));
              },
            },
            "Delete",
          ),
        ),
      );
      if (ready) {
        card.addEventListener("dblclick", () => this.onOpen(p.id));
      }
      this.list.append(card);
    }
  }
}
