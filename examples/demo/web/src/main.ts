import { catalogSummary, launchObservation, listStars, recomputeCatalog } from "./api.ts";
import { initTelemetry, tracedHandler } from "./otel.ts";

initTelemetry();

const FILE = "examples/demo/web/src/main.ts";
const output = document.querySelector<HTMLPreElement>("#output")!;

function show(value: unknown): void {
  output.textContent = typeof value === "string" ? value : JSON.stringify(value, null, 2);
}

async function busy(button: HTMLButtonElement, work: () => Promise<unknown>): Promise<void> {
  button.setAttribute("aria-busy", "true");
  button.disabled = true;
  try {
    show(await work());
  } catch (err) {
    show(`Error: ${(err as Error).message}`);
  } finally {
    button.disabled = false;
    button.removeAttribute("aria-busy");
  }
}

const onLaunch = tracedHandler("click Launch observation", { functionName: "onLaunch", filePath: FILE }, async () => {
  const star = document.querySelector<HTMLInputElement>("#star")!.value.trim();
  const site = document.querySelector<HTMLSelectElement>("#site")!.value;
  return launchObservation(star, site);
});

const onListStars = tracedHandler("click List stars", { functionName: "onListStars", filePath: FILE }, () =>
  listStars(),
);

const onRecompute = tracedHandler("click Recompute catalog", { functionName: "onRecompute", filePath: FILE }, () =>
  recomputeCatalog(),
);

const onSummary = tracedHandler("click Catalog summary", { functionName: "onSummary", filePath: FILE }, () =>
  catalogSummary(),
);

document.querySelector<HTMLFormElement>("#launch-form")!.addEventListener("submit", (e) => {
  e.preventDefault();
  void busy(document.querySelector<HTMLButtonElement>("#launch")!, onLaunch);
});
for (const [id, handler] of [
  ["#list-stars", onListStars],
  ["#recompute", onRecompute],
  ["#summary", onSummary],
] as const) {
  const button = document.querySelector<HTMLButtonElement>(id)!;
  button.addEventListener("click", () => void busy(button, handler));
}
