import { EDGE_COLORS, EFFECT_COLORS, EFFECT_ICONS, SYNTHETIC_COLORS } from "../graph/colors";
import type { ColorBy } from "../graph/scene";
import type { Store } from "../state";
import type { EdgeKind, EffectKind, NodeKind } from "../types";
import { EFFECT_KINDS } from "../types";
import { h } from "./dom";

const EDGE_LEGEND: { kind: EdgeKind; note: string }[] = [
  { kind: "calls", note: "thin" },
  { kind: "spawns", note: "dashed" },
  { kind: "requests", note: "arc" },
  { kind: "handles", note: "arrow" },
  { kind: "reads", note: "" },
  { kind: "writes", note: "" },
];

const SHAPES: { kind: NodeKind; glyph: string }[] = [
  { kind: "route", glyph: "◆" },
  { kind: "table", glyph: "⬬" },
  { kind: "host", glyph: "■" },
  { kind: "topic", glyph: "◯" },
  { kind: "dependency", glyph: "▲" },
];

export class Legend {
  readonly el: HTMLElement;

  constructor(
    private readonly store: Store,
    onColorBy: (mode: ColorBy) => void,
  ) {
    const effects = h(
      "div",
      { class: "fx-toggles", role: "group", "aria-label": "Effect kinds" },
      ...EFFECT_KINDS.map((k) => this.effectToggle(k)),
    );
    const transitive = h("input", {
      type: "checkbox",
      id: "lg-transitive",
      onchange: () => this.setTransitive(transitive.checked),
    });
    const colorBy = h(
      "select",
      { "aria-label": "Colour nodes by", onchange: () => onColorBy(colorBy.value as ColorBy) },
      h("option", { value: "effect" }, "Colour: effect"),
      h("option", { value: "package" }, "Colour: package"),
    );
    this.el = h(
      "aside",
      { class: "legend", "aria-label": "Legend" },
      h(
        "div",
        { class: "legend-row" },
        colorBy,
        h("label", { class: "check" }, transitive, " transitive effects"),
      ),
      effects,
      h(
        "div",
        { class: "legend-edges" },
        ...EDGE_LEGEND.map((e) =>
          h(
            "span",
            { class: "legend-item" },
            h("i", {
              class: `edge-swatch edge-${e.kind}`,
              style: { background: EDGE_COLORS[e.kind] },
            }),
            `${e.kind}${e.note ? ` · ${e.note}` : ""}`,
          ),
        ),
      ),
      h(
        "div",
        { class: "legend-shapes" },
        ...SHAPES.map((s) =>
          h(
            "span",
            { class: "legend-item" },
            h("i", { style: { color: SYNTHETIC_COLORS[s.kind] ?? "#fff" } }, s.glyph),
            s.kind,
          ),
        ),
        h("span", { class: "legend-item" }, h("i", { class: "halo" }), "entrypoint"),
        h("span", { class: "legend-item" }, h("i", { class: "dead-swatch" }), "dead"),
        h("span", { class: "legend-item" }, h("i", { class: "dim-swatch" }), "name/heuristic edge"),
      ),
    );
  }

  private effectToggle(kind: EffectKind): HTMLElement {
    const btn = h(
      "button",
      {
        class: "fx-toggle on",
        "aria-pressed": "true",
        style: { borderColor: EFFECT_COLORS[kind], color: EFFECT_COLORS[kind] },
        onclick: () => {
          const on = !this.store.effectFilter.has(kind);
          if (on) this.store.effectFilter.add(kind);
          else this.store.effectFilter.delete(kind);
          btn.classList.toggle("on", on);
          btn.setAttribute("aria-pressed", String(on));
          this.store.emit("view", undefined);
        },
      },
      `${EFFECT_ICONS[kind]} ${kind}`,
    );
    return btn;
  }

  private setTransitive(on: boolean): void {
    this.store.transitive = on;
    this.store.emit("view", undefined);
  }
}
