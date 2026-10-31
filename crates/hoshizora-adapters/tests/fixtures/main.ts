import { launchObservation, listStars } from "./api.ts";
import { initTelemetry, tracedHandler, wrap } from "./otel.ts";

initTelemetry();

const FILE = "main.ts";
const output = document.querySelector<HTMLPreElement>("#output")!;

function show(value: unknown): void {
  output.textContent = String(value);
}

/** Launch button. */
const onLaunch = tracedHandler("click Launch", { functionName: "onLaunch" }, async () => {
  const star = document.querySelector<HTMLInputElement>("#star")!.value.trim();
  return launchObservation(star);
});

export const onList = wrap(show);
const notCallable = compute(1, 2);

document.querySelector<HTMLFormElement>("#launch-form")!.addEventListener("submit", (e) => {
  e.preventDefault();
  void onLaunch();
});
output.onclick = onList;
for (const [id, handler] of [
  ["#a", onLaunch],
  ["#b", onList],
] as const) {
  listStars(id);
  document.querySelector(id)!.addEventListener("click", () => void handler());
}
