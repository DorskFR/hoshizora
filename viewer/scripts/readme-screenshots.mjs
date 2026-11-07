#!/usr/bin/env node
/**
 * Captures the README images from a running `hoshizora serve` holding the
 * examples/demo project, with fake OTLP traffic posted to /v1/traces.
 *
 * Usage (from the repo root):
 *   hoshizora serve --data-dir /tmp/hz --project examples/demo --port 5450 --otlp-port 5451
 *   cd viewer && npm run readme:shots [-- --url http://127.0.0.1:5450 --project demo]
 */
import { execFileSync } from "node:child_process";
import { mkdir, rm } from "node:fs/promises";
import { parseArgs } from "node:util";
import { chromium } from "@playwright/test";

const { values } = parseArgs({
  options: {
    url: { type: "string", default: "http://127.0.0.1:5450" },
    project: { type: "string", default: "demo" },
    out: { type: "string", default: "../docs/assets" },
  },
});
const { url, project, out } = values;
const VIEWPORT = { width: 1400, height: 860 };
const WEBGL_ARGS = [
  "--use-angle=swiftshader",
  "--enable-unsafe-swiftshader",
  "--ignore-gpu-blocklist",
];

const SERVICES = {
  web: "observatory-web",
  api: "observatory-api",
  worker: "observatory-worker",
};

const hex = (n) =>
  Array.from({ length: n }, () => Math.floor(Math.random() * 16).toString(16)).join("");
const str = (key, value) => ({ key, value: { stringValue: String(value) } });

function fn(service, name, file, start, dur, children = [], extra = []) {
  return {
    service,
    name,
    kind: 1,
    start,
    dur,
    children,
    attrs: [str("code.function.name", name), str("code.file.path", file), ...extra],
  };
}
function server(service, method, route, start, dur, children) {
  return {
    service,
    name: `${method} ${route}`,
    kind: 2,
    start,
    dur,
    children,
    attrs: [str("http.route", route), str("http.request.method", method)],
  };
}
function client(service, method, path, start, dur, children) {
  return {
    service,
    name: `${method}`,
    kind: 3,
    start,
    dur,
    children,
    attrs: [str("http.request.method", method), str("url.path", path)],
  };
}
function query(service, table, sql, start, dur) {
  return {
    service,
    name: sql.split(" ")[0],
    kind: 3,
    start,
    dur,
    children: [],
    attrs: [
      str("db.system.name", "postgresql"),
      str("db.collection.name", table),
      str("db.query.text", sql),
    ],
  };
}

const jitter = (ms) => ms * (0.7 + Math.random() * 0.6);

function launchObservation() {
  const weather = server(SERVICES.worker, "GET", "/weather/{site}", 0, jitter(60), [
    fn(SERVICES.worker, "weather", "worker/app/main.py", 1, jitter(55), [
      fn(SERVICES.worker, "sky_conditions", "worker/app/main.py", 2, jitter(40)),
      fn(SERVICES.worker, "seeing_index", "worker/app/main.py", 2, jitter(45), [
        query(
          SERVICES.worker,
          "observations",
          "SELECT seeing FROM observations WHERE site = $1",
          4,
          jitter(20),
        ),
      ]),
    ]),
  ]);
  return fn(SERVICES.web, "onLaunch", "web/src/main.ts", 0, 330, [
    fn(SERVICES.web, "launchObservation", "web/src/api.ts", 1, 320, [
      client(SERVICES.web, "POST", "/observations", 2, 315, [
        server(SERVICES.api, "POST", "/observations", 4, 300, [
          fn(SERVICES.api, "launch_observation", "api/src/handlers.rs", 5, 295, [
            fn(SERVICES.api, "star_by_name", "api/src/db.rs", 6, jitter(8), [
              query(
                SERVICES.api,
                "stars",
                "SELECT id, name FROM stars WHERE name = $1",
                7,
                jitter(5),
              ),
            ]),
            fn(SERVICES.api, "inventory", "api/src/db.rs", 16, jitter(10), [
              query(
                SERVICES.api,
                "inventory",
                "SELECT * FROM inventory WHERE site = $1",
                17,
                jitter(6),
              ),
            ]),
            fn(SERVICES.api, "fetch_weather", "api/src/worker_client.rs", 30, 70, [
              client(SERVICES.api, "GET", "/weather/north", 31, 68, [{ ...weather, start: 32 }]),
            ]),
            fn(SERVICES.api, "warm_lens", "api/src/catalog.rs", 105, jitter(120)),
            fn(SERVICES.api, "insert_observation", "api/src/db.rs", 240, jitter(25), [
              query(
                SERVICES.api,
                "observations",
                "INSERT INTO observations (star_id, site) VALUES ($1, $2)",
                242,
                jitter(18),
              ),
            ]),
            fn(SERVICES.api, "record_observation_event", "api/src/handlers.rs", 280, jitter(60), [
              fn(SERVICES.api, "insert_event", "api/src/db.rs", 300, jitter(12), [
                query(
                  SERVICES.api,
                  "events",
                  "INSERT INTO events (kind, payload) VALUES ($1, $2)",
                  301,
                  jitter(8),
                ),
              ]),
            ]),
          ]),
        ]),
      ]),
    ]),
  ]);
}

function listStars() {
  const perStar = Array.from({ length: 12 }, (_, i) =>
    fn(SERVICES.api, "observation_count_for_star", "api/src/db.rs", 12 + i * 6, jitter(5), [
      query(
        SERVICES.api,
        "observations",
        "SELECT count(*) FROM observations WHERE star_id = $1",
        13 + i * 6,
        jitter(4),
      ),
    ]),
  );
  return fn(SERVICES.web, "onListStars", "web/src/main.ts", 0, 100, [
    fn(SERVICES.web, "listStars", "web/src/api.ts", 1, 95, [
      client(SERVICES.web, "GET", "/stars", 2, 92, [
        server(SERVICES.api, "GET", "/stars", 3, 88, [
          fn(SERVICES.api, "list_stars", "api/src/handlers.rs", 4, 85, [
            fn(SERVICES.api, "all_stars", "api/src/db.rs", 5, jitter(6), [
              query(SERVICES.api, "stars", "SELECT id, name, magnitude FROM stars", 6, jitter(4)),
            ]),
            ...perStar,
          ]),
        ]),
      ]),
    ]),
  ]);
}

function recompute() {
  const magnitudes = Array.from({ length: 6 }, (_, i) =>
    fn(SERVICES.worker, "apparent_magnitude", "worker/app/photometry.py", 10 + i * 3, jitter(2)),
  );
  return fn(SERVICES.web, "onRecompute", "web/src/main.ts", 0, 200, [
    fn(SERVICES.web, "recomputeCatalog", "web/src/api.ts", 1, 195, [
      client(SERVICES.web, "POST", "/catalog/recompute", 2, 190, [
        server(SERVICES.api, "POST", "/catalog/recompute", 3, 185, [
          fn(SERVICES.api, "recompute_catalog", "api/src/handlers.rs", 4, 180, [
            fn(SERVICES.api, "all_stars", "api/src/db.rs", 5, jitter(6)),
            fn(SERVICES.api, "find_duplicate_stars", "api/src/catalog.rs", 12, jitter(30)),
            fn(SERVICES.api, "compute_magnitudes", "api/src/worker_client.rs", 45, 60, [
              client(SERVICES.api, "POST", "/compute/magnitudes", 46, 58, [
                server(SERVICES.worker, "POST", "/compute/magnitudes", 47, 55, [
                  fn(SERVICES.worker, "compute_magnitudes", "worker/app/main.py", 48, 52, [
                    fn(
                      SERVICES.worker,
                      "compute_all",
                      "worker/app/photometry.py",
                      49,
                      30,
                      magnitudes.map((m) => ({ ...m, start: m.start + 40 })),
                    ),
                    fn(SERVICES.worker, "publish_event", "worker/app/db.py", 82, jitter(10)),
                  ]),
                ]),
              ]),
            ]),
            fn(SERVICES.api, "cache_moduli", "api/src/handlers.rs", 130, jitter(20)),
          ]),
        ]),
      ]),
    ]),
  ]);
}

/** Flattens a tree into OTLP/JSON resourceSpans, one resource per service. */
function toOtlp(root) {
  const traceId = hex(32);
  const base = BigInt(Date.now() - 400) * 1_000_000n;
  const byService = new Map();
  const walk = (node, parentSpanId) => {
    const spanId = hex(16);
    const start = base + BigInt(Math.round(node.start * 1e6));
    const end = start + BigInt(Math.round(node.dur * 1e6));
    const span = {
      traceId,
      spanId,
      parentSpanId: parentSpanId ?? "",
      name: node.name,
      kind: node.kind,
      startTimeUnixNano: String(start),
      endTimeUnixNano: String(end),
      attributes: node.attrs,
    };
    if (!byService.has(node.service)) byService.set(node.service, []);
    byService.get(node.service).push(span);
    for (const child of node.children) walk(child, spanId);
  };
  walk(root);
  return {
    resourceSpans: [...byService].map(([service, spans]) => ({
      resource: { attributes: [str("service.name", service)] },
      scopeSpans: [{ scope: { name: "readme-screenshots" }, spans }],
    })),
  };
}

async function post(tree) {
  const res = await fetch(`${url}/v1/traces?project=${project}`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(toOtlp(tree)),
  });
  if (!res.ok) throw new Error(`POST /v1/traces: ${res.status} ${await res.text()}`);
}

const FLOWS = [launchObservation, listStars, launchObservation, recompute];
let flowIndex = 0;
const nextFlow = () => post(FLOWS[flowIndex++ % FLOWS.length]());

/** Keeps posting traces every `ms` until the returned stop function is called. */
function traffic(ms) {
  const timer = setInterval(() => nextFlow().catch((e) => console.error(e.message)), ms);
  return () => clearInterval(timer);
}

async function canvasIsBlank(page) {
  const png = await page.locator("canvas").first().screenshot();
  return new Set(png.subarray(png.length - 4000)).size < 8;
}

/** Screenshots, then reduces to a 256-colour PNG with ffmpeg to stay under 500 KB. */
async function shoot(page, file) {
  const path = `${out}/${file}`;
  const raw = `${path}.raw.png`;
  await page.addStyleTag({ content: ".fps { visibility: hidden; }" });
  await page.screenshot({ path: raw, timeout: 120_000 });
  execFileSync("ffmpeg", [
    "-y",
    "-loglevel",
    "error",
    "-i",
    raw,
    "-vf",
    "split[a][b];[a]palettegen=max_colors=256:stats_mode=full[p];[b][p]paletteuse=dither=sierra2_4a",
    path,
  ]);
  await rm(raw);
  console.log(path);
}

async function openMap(context, select) {
  const page = await context.newPage();
  const query = select ? `&select=${encodeURIComponent(select)}` : "";
  await page.goto(`${url}/?level=functions&live=1${query}#/p/${project}`, {
    waitUntil: "networkidle",
  });
  await page.waitForSelector("canvas");
  await page.selectOption('[aria-label="Heat overlay"]', "self");
  await page.waitForTimeout(9000);
  if (await canvasIsBlank(page)) throw new Error("WebGL canvas is blank");
  return page;
}

await mkdir(out, { recursive: true });
for (let i = 0; i < 24; i++) await nextFlow();

const browser = await chromium.launch({ args: WEBGL_ARGS });
const contextOptions = { viewport: VIEWPORT, colorScheme: "dark" };
try {
  const heroContext = await browser.newContext(contextOptions);
  const map = await openMap(heroContext, "fn:api/src/handlers.rs#launch_observation");
  const stop = traffic(1200);
  await map.waitForTimeout(4000);
  await shoot(map, "hero.png");
  stop();
  await heroContext.close();

  const storyContext = await browser.newContext(contextOptions);
  const story = await storyContext.newPage();
  const entry = encodeURIComponent("fn:web/src/main.ts#onLaunch");
  await story.goto(`${url}/#/p/${project}/story/${entry}`, { waitUntil: "networkidle" });
  await story.waitForSelector(".story-head");
  await story.waitForTimeout(3000);
  await shoot(story, "story.png");
  await storyContext.close();
} finally {
  await browser.close();
}
