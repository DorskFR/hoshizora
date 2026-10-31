type Child = Node | string | number | null | undefined | false | Child[];

type Props<K extends keyof HTMLElementTagNameMap> = Partial<
  Omit<HTMLElementTagNameMap[K], "style" | "children" | "dataset">
> & {
  class?: string;
  style?: Partial<CSSStyleDeclaration>;
  dataset?: Record<string, string>;
  onclick?: (ev: MouseEvent) => void;
  oninput?: (ev: Event) => void;
  onchange?: (ev: Event) => void;
  onkeydown?: (ev: KeyboardEvent) => void;
  "aria-label"?: string;
  "aria-pressed"?: string;
  "aria-live"?: string;
  role?: string;
};

export function h<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  props: Props<K> | null = null,
  ...children: Child[]
): HTMLElementTagNameMap[K] {
  const el = document.createElement(tag);
  if (props) {
    for (const [k, v] of Object.entries(props)) {
      if (v === undefined || v === null) continue;
      if (k === "class") el.className = v as string;
      else if (k === "style") Object.assign(el.style, v as Partial<CSSStyleDeclaration>);
      else if (k === "dataset") Object.assign(el.dataset, v as Record<string, string>);
      else if (k.startsWith("aria-") || k === "role") el.setAttribute(k, String(v));
      else if (k.startsWith("on") && typeof v === "function")
        el.addEventListener(k.slice(2), v as EventListener);
      else (el as unknown as Record<string, unknown>)[k] = v;
    }
  }
  append(el, children);
  return el;
}

export function append(el: Node, children: Child[]): void {
  for (const c of children) {
    if (c === null || c === undefined || c === false) continue;
    if (Array.isArray(c)) append(el, c);
    else el.appendChild(typeof c === "object" ? c : document.createTextNode(String(c)));
  }
}

export function clear(el: Element): void {
  el.replaceChildren();
}

export function fmtNanos(n: number): string {
  if (n < 1_000) return `${n}ns`;
  if (n < 1_000_000) return `${(n / 1_000).toFixed(1)}µs`;
  if (n < 1_000_000_000) return `${(n / 1_000_000).toFixed(1)}ms`;
  return `${(n / 1_000_000_000).toFixed(2)}s`;
}

export function fmtMs(ms: number): string {
  return ms < 1
    ? `${(ms * 1000).toFixed(0)}µs`
    : ms < 1000
      ? `${ms.toFixed(1)}ms`
      : `${(ms / 1000).toFixed(2)}s`;
}

export function fmtRate(r: number): string {
  return r >= 100
    ? `${r.toFixed(0)}/s`
    : r >= 1
      ? `${r.toFixed(1)}/s`
      : `${(r * 60).toFixed(1)}/min`;
}

export function shortId(id: string): string {
  const hash = id.indexOf("#");
  return hash >= 0 ? id.slice(hash + 1) : id.slice(id.indexOf(":") + 1);
}
