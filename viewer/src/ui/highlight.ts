import type { Language } from "../types";

type Highlighter = Awaited<ReturnType<typeof import("shiki/core").createHighlighterCore>>;

let pending: Promise<Highlighter> | null = null;

function load(): Promise<Highlighter> {
  if (!pending) {
    pending = Promise.all([import("shiki/core"), import("shiki/engine/javascript")]).then(
      ([core, engine]) =>
        core.createHighlighterCore({
          themes: [import("@shikijs/themes/vitesse-dark")],
          langs: [
            import("@shikijs/langs/rust"),
            import("@shikijs/langs/python"),
            import("@shikijs/langs/typescript"),
            import("@shikijs/langs/tsx"),
            import("@shikijs/langs/javascript"),
            import("@shikijs/langs/go"),
          ],
          engine: engine.createJavaScriptRegexEngine(),
        }),
    );
  }
  return pending;
}

export function shikiLang(lang: Language | null, file: string | null): string {
  if (file?.endsWith(".tsx") || file?.endsWith(".jsx")) return "tsx";
  if (file?.endsWith(".svelte") || file?.endsWith(".vue")) return "typescript";
  switch (lang) {
    case "rust":
      return "rust";
    case "python":
      return "python";
    case "typescript":
      return "typescript";
    case "javascript":
      return "javascript";
    case "go":
      return "go";
    default:
      return "text";
  }
}

export async function highlight(code: string, lang: string, startLine: number): Promise<string> {
  const hl = await load();
  return hl.codeToHtml(code, {
    lang: hl.getLoadedLanguages().includes(lang) ? lang : "text",
    theme: "vitesse-dark",
    transformers: [
      {
        pre(node) {
          node.properties.style = `${node.properties.style ?? ""};--start:${startLine - 1}`;
        },
      },
    ],
  });
}
