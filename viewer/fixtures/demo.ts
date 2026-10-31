import type {
  Edge,
  EdgeKind,
  Effect,
  EffectKind,
  EntrypointKind,
  Graph,
  GraphNode,
  Insight,
  Language,
  NodeKind,
  NodeStats,
  RuntimeSpan,
  TraceSummary,
} from "../src/types";

const ROOT = "/home/dev/shop";

interface FnOpts {
  file: string;
  name: string;
  lang: Language;
  lines: [number, number];
  signature: string;
  params?: { name: string; ty?: string; default?: string }[];
  returns?: string;
  doc?: string;
  isAsync?: boolean;
  attributes?: string[];
  effects?: Partial<Effect>[];
  transitive?: EffectKind[];
  entrypoint?: EntrypointKind;
  dead?: boolean;
  cyclomatic?: number;
  loops?: number;
  maxLoopDepth?: number;
  time?: string;
  basis?: string[];
  parent?: string;
  fanIn?: number;
  fanOut?: number;
}

function fn(o: FnOpts): GraphNode {
  const direct: Effect[] = (o.effects ?? []).map((e) => ({
    kind: e.kind ?? "network",
    target: e.target ?? null,
    via: e.via ?? "",
    line: e.line ?? o.lines[0],
    loop_depth: e.loop_depth ?? 0,
    awaited: e.awaited ?? false,
    rule: e.rule ?? "",
  }));
  const transitive = Array.from(new Set([...(o.transitive ?? []), ...direct.map((e) => e.kind)]));
  const dir = o.file.split("/")[0]!;
  const module =
    o.lang === "rust"
      ? `${dir === "crates" ? o.file.split("/")[1] : dir}::${o.file.split("/").pop()!.replace(".rs", "")}`
      : o.file.replace(/\.(ts|tsx|py)$/, "").replace(/\//g, o.lang === "python" ? "." : "/");
  return {
    id: `fn:${o.file}#${o.name}`,
    kind: "function",
    name: o.name.split(/::|\./).pop()!,
    qualified_name: `${module}${o.lang === "rust" ? "::" : "."}${o.name}`,
    language: o.lang,
    file: o.file,
    span: { start_line: o.lines[0], end_line: o.lines[1] },
    parent: o.parent ?? `file:${o.file}`,
    function: {
      signature: o.signature,
      params: (o.params ?? []).map((p) => ({
        name: p.name,
        ty: p.ty ?? null,
        default: p.default ?? null,
      })),
      returns: o.returns ?? null,
      doc: o.doc ?? null,
      is_async: o.isAsync ?? false,
      is_method: o.parent?.startsWith("type:") ?? false,
      visibility: "public",
      attributes: o.attributes ?? [],
      cyclomatic: o.cyclomatic ?? 1,
      branches: Math.max(0, (o.cyclomatic ?? 1) - 1),
      loops: o.loops ?? 0,
      max_loop_depth: o.maxLoopDepth ?? 0,
      recursive: false,
      complexity: {
        time: o.time ?? "O(1)",
        space: "O(1)",
        basis: o.basis ?? ["no loops", "no recursion"],
      },
      fan_in: o.fanIn ?? 0,
      fan_out: o.fanOut ?? 0,
    },
    effects: { direct, transitive },
    entrypoint: o.entrypoint ?? null,
    dead: o.dead ?? false,
    lines: o.lines[1] - o.lines[0] + 1,
  };
}

function container(
  kind: NodeKind,
  id: string,
  name: string,
  qualified: string,
  lang: Language | null,
  parent: string | null,
  lines: number,
  file: string | null = null,
): GraphNode {
  return {
    id,
    kind,
    name,
    qualified_name: qualified,
    language: lang,
    file,
    span: null,
    parent,
    function: null,
    effects: { direct: [], transitive: [] },
    entrypoint: null,
    dead: false,
    lines,
  };
}

function synthetic(kind: NodeKind, id: string, name: string): GraphNode {
  return container(kind, id, name, name, null, null, 0);
}

function edge(
  from: string,
  to: string,
  kind: EdgeKind,
  confidence: Edge["confidence"] = "exact",
  extra: Partial<Edge> = {},
): Edge {
  return {
    from,
    to,
    kind,
    confidence,
    line: null,
    loop_depth: 0,
    conditional: false,
    awaited: false,
    attrs: {},
    ...extra,
  };
}

const F = {
  checkout: "webui/src/pages/Checkout.tsx",
  client: "webui/src/api/client.ts",
  main: "crates/api/src/main.rs",
  orders: "crates/api/src/orders.rs",
  db: "crates/api/src/db.rs",
  pyMain: "worker/app/main.py",
  inventory: "worker/app/inventory.py",
  utils: "worker/app/utils.py",
};

const ID = {
  onSubmit: `fn:${F.checkout}#onSubmit`,
  renderCart: `fn:${F.checkout}#renderCart`,
  createOrder: `fn:${F.client}#createOrder`,
  request: `fn:${F.client}#request`,
  main: `fn:${F.main}#main`,
  router: `fn:${F.main}#router`,
  Order: `type:${F.orders}#Order`,
  create_order: `fn:${F.orders}#create_order`,
  validate: `fn:${F.orders}#Order::validate`,
  reserve_stock: `fn:${F.orders}#reserve_stock`,
  send_confirmation: `fn:${F.orders}#send_confirmation`,
  test_creates: `fn:${F.orders}#tests::creates_order`,
  insert_order: `fn:${F.db}#insert_order`,
  pool: `fn:${F.db}#pool`,
  reserve: `fn:${F.pyMain}#reserve`,
  nightly: `fn:${F.pyMain}#nightly_reconcile`,
  check_inventory: `fn:${F.inventory}#check_inventory`,
  decrement: `fn:${F.inventory}#decrement`,
  legacy_format: `fn:${F.utils}#legacy_format`,
  fmt_money: `fn:${F.utils}#fmt_money`,
  routeOrders: "route:POST /api/orders",
  routeReserve: "route:POST /reserve",
  tOrders: "table:orders",
  tInventory: "table:inventory",
  hostSmtp: "host:smtp.example.com",
  extSqlx: "ext:sqlx",
  extReqwest: "ext:reqwest",
  extFastapi: "ext:fastapi",
};

const nodes: GraphNode[] = [
  container("package", "mod:webui", "webui", "webui", "typescript", null, 180),
  container("package", "mod:crates/api", "api", "crates/api", "rust", null, 260),
  container("package", "mod:worker", "worker", "worker", "python", null, 140),

  container(
    "file",
    `file:${F.checkout}`,
    "Checkout.tsx",
    F.checkout,
    "typescript",
    "mod:webui",
    120,
    F.checkout,
  ),
  container(
    "file",
    `file:${F.client}`,
    "client.ts",
    F.client,
    "typescript",
    "mod:webui",
    60,
    F.client,
  ),
  container("file", `file:${F.main}`, "main.rs", F.main, "rust", "mod:crates/api", 50, F.main),
  container(
    "file",
    `file:${F.orders}`,
    "orders.rs",
    F.orders,
    "rust",
    "mod:crates/api",
    150,
    F.orders,
  ),
  container("file", `file:${F.db}`, "db.rs", F.db, "rust", "mod:crates/api", 60, F.db),
  container("file", `file:${F.pyMain}`, "main.py", F.pyMain, "python", "mod:worker", 60, F.pyMain),
  container(
    "file",
    `file:${F.inventory}`,
    "inventory.py",
    F.inventory,
    "python",
    "mod:worker",
    50,
    F.inventory,
  ),
  container("file", `file:${F.utils}`, "utils.py", F.utils, "python", "mod:worker", 30, F.utils),

  container(
    "type",
    ID.Order,
    "Order",
    "api::orders::Order",
    "rust",
    `file:${F.orders}`,
    30,
    F.orders,
  ),

  fn({
    file: F.checkout,
    name: "onSubmit",
    lang: "typescript",
    lines: [41, 58],
    signature: "async function onSubmit(ev: FormEvent): Promise<void>",
    params: [{ name: "ev", ty: "FormEvent" }],
    returns: "Promise<void>",
    doc: "Submit the cart as a new order.",
    isAsync: true,
    attributes: ["onSubmit"],
    entrypoint: "ui_handler",
    transitive: ["network"],
    cyclomatic: 3,
    fanOut: 2,
  }),
  fn({
    file: F.checkout,
    name: "renderCart",
    lang: "typescript",
    lines: [12, 39],
    signature: "function renderCart(items: CartItem[]): JSX.Element",
    params: [{ name: "items", ty: "CartItem[]" }],
    returns: "JSX.Element",
    loops: 1,
    maxLoopDepth: 1,
    time: "O(n)",
    basis: ["one loop over items"],
    fanIn: 1,
  }),
  fn({
    file: F.client,
    name: "createOrder",
    lang: "typescript",
    lines: [22, 31],
    signature: "export async function createOrder(input: OrderInput): Promise<Order>",
    params: [{ name: "input", ty: "OrderInput" }],
    returns: "Promise<Order>",
    doc: "POST the order to the API.",
    isAsync: true,
    effects: [
      {
        kind: "network",
        target: "/api/orders",
        via: "request",
        line: 26,
        awaited: true,
        rule: "ts.fetch",
      },
    ],
    fanIn: 1,
    fanOut: 1,
  }),
  fn({
    file: F.client,
    name: "request",
    lang: "typescript",
    lines: [5, 20],
    signature:
      "async function request<T>(method: string, path: string, body?: unknown): Promise<T>",
    params: [
      { name: "method", ty: "string" },
      { name: "path", ty: "string" },
      { name: "body", ty: "unknown", default: "undefined" },
    ],
    returns: "Promise<T>",
    isAsync: true,
    effects: [
      { kind: "network", target: null, via: "fetch", line: 9, awaited: true, rule: "ts.fetch" },
    ],
    cyclomatic: 3,
    fanIn: 1,
  }),
  fn({
    file: F.main,
    name: "main",
    lang: "rust",
    lines: [8, 24],
    signature: "async fn main() -> anyhow::Result<()>",
    returns: "anyhow::Result<()>",
    isAsync: true,
    attributes: ["tokio::main"],
    entrypoint: "main",
    effects: [
      {
        kind: "network",
        target: "0.0.0.0:8080",
        via: "axum::serve",
        line: 22,
        awaited: true,
        rule: "rust.axum.serve",
      },
    ],
    transitive: ["db_write", "db_read", "spawn"],
    fanOut: 2,
  }),
  fn({
    file: F.main,
    name: "router",
    lang: "rust",
    lines: [26, 40],
    signature: "fn router(state: AppState) -> Router",
    params: [{ name: "state", ty: "AppState" }],
    returns: "Router",
    fanIn: 1,
    fanOut: 1,
  }),
  fn({
    file: F.orders,
    name: "create_order",
    lang: "rust",
    lines: [34, 72],
    signature:
      "pub async fn create_order(State(state): State<AppState>, Json(input): Json<OrderInput>) -> Result<Json<Order>, ApiError>",
    params: [
      { name: "state", ty: "State<AppState>" },
      { name: "input", ty: "Json<OrderInput>" },
    ],
    returns: "Result<Json<Order>, ApiError>",
    doc: "Handle `POST /api/orders`: validate, persist, reserve stock on the worker, confirm by mail.",
    isAsync: true,
    attributes: ["axum::debug_handler"],
    entrypoint: "http_handler",
    effects: [
      {
        kind: "spawn",
        target: "send_confirmation",
        via: "tokio::spawn",
        line: 66,
        rule: "rust.tokio.spawn",
      },
    ],
    transitive: ["db_write", "network", "lock"],
    cyclomatic: 5,
    fanIn: 1,
    fanOut: 4,
  }),
  fn({
    file: F.orders,
    name: "Order::validate",
    lang: "rust",
    lines: [20, 32],
    signature: "pub fn validate(&self) -> Result<(), ValidationError>",
    returns: "Result<(), ValidationError>",
    parent: ID.Order,
    cyclomatic: 4,
    loops: 1,
    maxLoopDepth: 1,
    time: "O(n)",
    basis: ["one loop over line items"],
    fanIn: 1,
  }),
  fn({
    file: F.orders,
    name: "reserve_stock",
    lang: "rust",
    lines: [74, 96],
    signature:
      "async fn reserve_stock(client: &reqwest::Client, order: &Order) -> Result<(), ApiError>",
    params: [
      { name: "client", ty: "&reqwest::Client" },
      { name: "order", ty: "&Order" },
    ],
    returns: "Result<(), ApiError>",
    isAsync: true,
    effects: [
      {
        kind: "network",
        target: "http://worker:9000/reserve",
        via: "client.post",
        line: 80,
        awaited: true,
        rule: "rust.reqwest.post",
      },
    ],
    cyclomatic: 3,
    fanIn: 1,
  }),
  fn({
    file: F.orders,
    name: "send_confirmation",
    lang: "rust",
    lines: [98, 118],
    signature: "async fn send_confirmation(mailer: Mailer, order: Order)",
    params: [
      { name: "mailer", ty: "Mailer" },
      { name: "order", ty: "Order" },
    ],
    isAsync: true,
    effects: [
      {
        kind: "lock",
        target: "mailer.inner",
        via: "mailer.inner.lock",
        line: 101,
        awaited: true,
        rule: "rust.tokio.mutex",
      },
      {
        kind: "network",
        target: "smtp.example.com",
        via: "lettre::send",
        line: 110,
        awaited: true,
        rule: "rust.lettre.send",
      },
    ],
    fanIn: 1,
  }),
  fn({
    file: F.orders,
    name: "tests::creates_order",
    lang: "rust",
    lines: [124, 148],
    signature: "async fn creates_order()",
    isAsync: true,
    attributes: ["tokio::test"],
    entrypoint: "test",
    transitive: ["db_write", "network", "spawn"],
    fanOut: 1,
  }),
  fn({
    file: F.db,
    name: "insert_order",
    lang: "rust",
    lines: [18, 40],
    signature: "pub async fn insert_order(pool: &PgPool, order: &Order) -> sqlx::Result<i64>",
    params: [
      { name: "pool", ty: "&PgPool" },
      { name: "order", ty: "&Order" },
    ],
    returns: "sqlx::Result<i64>",
    doc: "Insert the order row and return its id.",
    isAsync: true,
    effects: [
      {
        kind: "db_write",
        target: "orders",
        via: "sqlx::query",
        line: 22,
        awaited: true,
        rule: "rust.sqlx.query",
      },
    ],
    fanIn: 2,
  }),
  fn({
    file: F.db,
    name: "pool",
    lang: "rust",
    lines: [6, 16],
    signature: "pub async fn pool(url: &str) -> sqlx::Result<PgPool>",
    params: [{ name: "url", ty: "&str" }],
    returns: "sqlx::Result<PgPool>",
    isAsync: true,
    effects: [
      {
        kind: "network",
        target: "postgres",
        via: "PgPoolOptions::connect",
        line: 12,
        awaited: true,
        rule: "rust.sqlx.connect",
      },
    ],
    fanIn: 1,
  }),
  fn({
    file: F.pyMain,
    name: "reserve",
    lang: "python",
    lines: [18, 36],
    signature: "async def reserve(req: ReserveRequest) -> ReserveResponse",
    params: [{ name: "req", ty: "ReserveRequest" }],
    returns: "ReserveResponse",
    doc: "Reserve stock for every line of an order.",
    isAsync: true,
    attributes: ['app.post("/reserve")'],
    entrypoint: "http_handler",
    transitive: ["db_read", "db_write"],
    cyclomatic: 3,
    loops: 1,
    maxLoopDepth: 1,
    time: "O(n)",
    basis: ["one loop over req.items", "I/O inside the loop"],
    fanIn: 1,
    fanOut: 3,
  }),
  fn({
    file: F.pyMain,
    name: "nightly_reconcile",
    lang: "python",
    lines: [40, 58],
    signature: "async def nightly_reconcile() -> None",
    isAsync: true,
    attributes: ['scheduler.scheduled_job("cron", hour=3)'],
    entrypoint: "job",
    transitive: ["db_read", "sleep"],
    effects: [
      {
        kind: "sleep",
        target: null,
        via: "asyncio.sleep",
        line: 55,
        awaited: true,
        rule: "py.asyncio.sleep",
      },
    ],
    loops: 1,
    maxLoopDepth: 1,
    time: "O(n)",
    basis: ["one loop over skus"],
    fanOut: 1,
  }),
  fn({
    file: F.inventory,
    name: "check_inventory",
    lang: "python",
    lines: [10, 24],
    signature: "async def check_inventory(db: Database, sku: str) -> int",
    params: [
      { name: "db", ty: "Database" },
      { name: "sku", ty: "str" },
    ],
    returns: "int",
    doc: "Return the available quantity for a SKU.",
    isAsync: true,
    effects: [
      {
        kind: "db_read",
        target: "inventory",
        via: "db.fetch_one",
        line: 14,
        awaited: true,
        rule: "py.databases.fetch",
      },
    ],
    fanIn: 2,
  }),
  fn({
    file: F.inventory,
    name: "decrement",
    lang: "python",
    lines: [26, 44],
    signature: "async def decrement(db: Database, sku: str, qty: int) -> None",
    params: [
      { name: "db", ty: "Database" },
      { name: "sku", ty: "str" },
      { name: "qty", ty: "int" },
    ],
    isAsync: true,
    effects: [
      {
        kind: "db_write",
        target: "inventory",
        via: "db.execute",
        line: 30,
        awaited: true,
        rule: "py.databases.execute",
      },
    ],
    fanIn: 1,
  }),
  fn({
    file: F.utils,
    name: "legacy_format",
    lang: "python",
    lines: [4, 14],
    signature: "def legacy_format(order: dict) -> str",
    params: [{ name: "order", ty: "dict" }],
    returns: "str",
    doc: "Old plain-text order rendering. Nothing calls this any more.",
    dead: true,
    loops: 2,
    maxLoopDepth: 2,
    time: "O(n^2)",
    basis: ["nested loops over items x fields"],
  }),
  fn({
    file: F.utils,
    name: "fmt_money",
    lang: "python",
    lines: [16, 22],
    signature: "def fmt_money(cents: int, currency: str = 'USD') -> str",
    params: [
      { name: "cents", ty: "int" },
      { name: "currency", ty: "str", default: "'USD'" },
    ],
    returns: "str",
    fanIn: 1,
  }),

  synthetic("route", ID.routeOrders, "POST /api/orders"),
  synthetic("route", ID.routeReserve, "POST /reserve"),
  synthetic("table", ID.tOrders, "orders"),
  synthetic("table", ID.tInventory, "inventory"),
  synthetic("host", ID.hostSmtp, "smtp.example.com"),
  synthetic("dependency", ID.extSqlx, "sqlx"),
  synthetic("dependency", ID.extReqwest, "reqwest"),
  synthetic("dependency", ID.extFastapi, "fastapi"),
];

const contains: Edge[] = nodes
  .filter((n) => n.parent)
  .map((n) => edge(n.parent!, n.id, "contains"));

const edges: Edge[] = [
  ...contains,
  edge(ID.onSubmit, ID.createOrder, "calls", "exact", { line: 48, awaited: true }),
  edge(ID.onSubmit, ID.renderCart, "calls", "exact", { line: 44 }),
  edge(ID.createOrder, ID.request, "calls", "exact", { line: 26, awaited: true }),
  edge(ID.createOrder, ID.routeOrders, "requests", "heuristic", { line: 26 }),
  edge(ID.routeOrders, ID.create_order, "handles", "exact"),
  edge(ID.main, ID.router, "calls", "exact", { line: 18 }),
  edge(ID.main, ID.pool, "calls", "exact", { line: 14, awaited: true }),
  edge(ID.router, ID.create_order, "calls", "name", { line: 31 }),
  edge(ID.create_order, ID.validate, "calls", "exact", { line: 40 }),
  edge(ID.create_order, ID.insert_order, "calls", "exact", { line: 48, awaited: true }),
  edge(ID.create_order, ID.reserve_stock, "calls", "exact", { line: 55, awaited: true }),
  edge(ID.create_order, ID.send_confirmation, "spawns", "exact", { line: 66 }),
  edge(ID.insert_order, ID.tOrders, "writes", "heuristic", { line: 22 }),
  edge(ID.reserve_stock, ID.routeReserve, "requests", "heuristic", { line: 80 }),
  edge(ID.send_confirmation, ID.hostSmtp, "requests", "heuristic", { line: 110 }),
  edge(ID.test_creates, ID.create_order, "calls", "exact", { line: 130, awaited: true }),
  edge(ID.routeReserve, ID.reserve, "handles", "exact"),
  edge(ID.reserve, ID.check_inventory, "calls", "exact", {
    line: 26,
    loop_depth: 1,
    awaited: true,
  }),
  edge(ID.reserve, ID.decrement, "calls", "exact", {
    line: 30,
    loop_depth: 1,
    awaited: true,
    conditional: true,
  }),
  edge(ID.reserve, ID.fmt_money, "calls", "name", { line: 34 }),
  edge(ID.nightly, ID.check_inventory, "calls", "exact", {
    line: 48,
    loop_depth: 1,
    awaited: true,
  }),
  edge(ID.check_inventory, ID.tInventory, "reads", "heuristic", { line: 14 }),
  edge(ID.decrement, ID.tInventory, "writes", "heuristic", { line: 30 }),
  edge(`file:${F.db}`, ID.extSqlx, "uses"),
  edge(`file:${F.orders}`, ID.extReqwest, "uses"),
  edge(`file:${F.pyMain}`, ID.extFastapi, "uses"),
  edge(`file:${F.client}`, `file:${F.checkout}`, "imports", "exact"),
];

export const demoGraph: Graph = {
  meta: {
    version: 1,
    root: ROOT,
    generated_at: "2026-10-31T09:00:00Z",
    languages: ["typescript", "rust", "python"],
    files: 8,
    warnings: ["worker/app/legacy.py: unresolved import `six`"],
  },
  nodes,
  edges,
};

export const demoSources: Record<string, string> = {
  [F.checkout]: `import { useState } from "react";
import { createOrder } from "../api/client";
import type { CartItem, OrderInput } from "../types";

export interface CheckoutProps {
  items: CartItem[];
  onDone(orderId: number): void;
}

const money = (c: number) => (c / 100).toFixed(2);

function renderCart(items: CartItem[]): JSX.Element {
  let total = 0;
  const rows = [];
  for (const item of items) {
    total += item.priceCents * item.qty;
    rows.push(
      <tr key={item.sku}>
        <td>{item.name}</td>
        <td>{item.qty}</td>
        <td>{money(item.priceCents)}</td>
      </tr>,
    );
  }
  return (
    <table className="cart">
      <tbody>{rows}</tbody>
      <tfoot>
        <tr>
          <td colSpan={2}>Total</td>
          <td>{money(total)}</td>
        </tr>
      </tfoot>
    </table>
  );
}

export function Checkout({ items, onDone }: CheckoutProps) {
  const [busy, setBusy] = useState(false);

  async function onSubmit(ev: FormEvent): Promise<void> {
    ev.preventDefault();
    if (busy || items.length === 0) return;
    renderCart(items);
    setBusy(true);
    try {
      const input: OrderInput = { items: items.map((i) => ({ sku: i.sku, qty: i.qty })) };
      const order = await createOrder(input);
      onDone(order.id);
    } catch (err) {
      console.error("checkout failed", err);
    } finally {
      setBusy(false);
    }
  }

  return <form onSubmit={onSubmit}>{renderCart(items)}<button disabled={busy}>Pay</button></form>;
}
`,
  [F.client]: `import type { Order, OrderInput } from "../types";

const BASE = import.meta.env.VITE_API ?? "";

async function request<T>(method: string, path: string, body?: unknown): Promise<T> {
  const res = await fetch(BASE + path, {
    method,
    headers: { "content-type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  if (!res.ok) {
    throw new Error(\`\${method} \${path} failed: \${res.status}\`);
  }
  if (res.status === 204) {
    return undefined as T;
  }
  return (await res.json()) as T;
}

/** POST the order to the API. */
export async function createOrder(input: OrderInput): Promise<Order> {
  return request<Order>("POST", "/api/orders", input);
}
`,
  [F.main]: `mod db;
mod orders;

use axum::Router;
use std::net::SocketAddr;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().init();
    let url = std::env::var("DATABASE_URL")?;
    let pool = db::pool(&url).await?;
    let state = AppState {
        pool,
        http: reqwest::Client::new(),
        mailer: Mailer::from_env()?,
    };
    let app = router(state);
    let addr: SocketAddr = "0.0.0.0:8080".parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/orders", axum::routing::post(orders::create_order))
        .with_state(state)
}
`,
  [F.orders]: `use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};

use crate::db;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Order {
    pub id: i64,
    pub items: Vec<LineItem>,
    pub total_cents: i64,
}

impl Order {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.items.is_empty() {
            return Err(ValidationError::Empty);
        }
        for item in &self.items {
            if item.qty == 0 {
                return Err(ValidationError::ZeroQty(item.sku.clone()));
            }
        }
        Ok(())
    }
}

/// Handle \`POST /api/orders\`: validate, persist, reserve stock on the worker, confirm by mail.
#[axum::debug_handler]
pub async fn create_order(
    State(state): State<AppState>,
    Json(input): Json<OrderInput>,
) -> Result<Json<Order>, ApiError> {
    let mut order = Order::from(input);
    order.validate()?;
    let id = db::insert_order(&state.pool, &order).await?;
    order.id = id;
    reserve_stock(&state.http, &order).await?;
    let mailer = state.mailer.clone();
    let copy = order.clone();
    tokio::spawn(send_confirmation(mailer, copy));
    Ok(Json(order))
}

async fn reserve_stock(client: &reqwest::Client, order: &Order) -> Result<(), ApiError> {
    let body = ReserveRequest::from(order);
    let res = client
        .post("http://worker:9000/reserve")
        .json(&body)
        .send()
        .await?;
    if !res.status().is_success() {
        return Err(ApiError::Upstream(res.status()));
    }
    Ok(())
}

async fn send_confirmation(mailer: Mailer, order: Order) {
    let guard = mailer.inner.lock().await;
    let msg = guard.render(&order);
    if let Err(err) = lettre::send(&guard.transport, msg).await {
        tracing::warn!(?err, "confirmation mail failed");
    }
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn creates_order() {
        let state = test_state().await;
        let res = create_order(State(state), Json(sample_input())).await.unwrap();
        assert!(res.0.id > 0);
    }
}
`,
  [F.db]: `use sqlx::postgres::{PgPool, PgPoolOptions};

use crate::orders::Order;

pub async fn pool(url: &str) -> sqlx::Result<PgPool> {
    PgPoolOptions::new().max_connections(8).connect(url).await
}

/// Insert the order row and return its id.
pub async fn insert_order(pool: &PgPool, order: &Order) -> sqlx::Result<i64> {
    let row: (i64,) = sqlx::query_as(
        "INSERT INTO orders (total_cents, items) VALUES ($1, $2) RETURNING id",
    )
    .bind(order.total_cents)
    .bind(serde_json::to_value(&order.items).unwrap())
    .fetch_one(pool)
    .await?;
    Ok(row.0)
}
`,
  [F.pyMain]: `import asyncio

from fastapi import FastAPI
from apscheduler.schedulers.asyncio import AsyncIOScheduler

from .inventory import check_inventory, decrement
from .utils import fmt_money
from .models import ReserveRequest, ReserveResponse

app = FastAPI()
scheduler = AsyncIOScheduler()


@app.post("/reserve")
async def reserve(req: ReserveRequest) -> ReserveResponse:
    """Reserve stock for every line of an order."""
    reserved = []
    for item in req.items:
        available = await check_inventory(db, item.sku)
        if available >= item.qty:
            await decrement(db, item.sku, item.qty)
            reserved.append(item.sku)
    total = fmt_money(req.total_cents)
    return ReserveResponse(reserved=reserved, total=total)


@scheduler.scheduled_job("cron", hour=3)
async def nightly_reconcile() -> None:
    for sku in await all_skus(db):
        qty = await check_inventory(db, sku)
        if qty < 0:
            log.warning("negative stock for %s", sku)
        await asyncio.sleep(0.05)
`,
  [F.inventory]: `from databases import Database


async def check_inventory(db: Database, sku: str) -> int:
    """Return the available quantity for a SKU."""
    row = await db.fetch_one(
        "SELECT quantity FROM inventory WHERE sku = :sku", {"sku": sku}
    )
    return row["quantity"] if row else 0


async def decrement(db: Database, sku: str, qty: int) -> None:
    await db.execute(
        "UPDATE inventory SET quantity = quantity - :qty WHERE sku = :sku",
        {"sku": sku, "qty": qty},
    )
`,
  [F.utils]: `def legacy_format(order: dict) -> str:
    """Old plain-text order rendering. Nothing calls this any more."""
    lines = []
    for item in order["items"]:
        for key in item:
            lines.append(f"{key}={item[key]}")
    return "\\n".join(lines)


def fmt_money(cents: int, currency: str = "USD") -> str:
    return f"{cents / 100:.2f} {currency}"
`,
};

const MS = 1_000_000;

interface SpanSpec {
  id: string;
  parent: string | null;
  service: string;
  name: string;
  kind: RuntimeSpan["kind"];
  node: string | null;
  start: number;
  end: number;
  attrs?: Record<string, string>;
}

export const demoFlowSpans: SpanSpec[] = [
  {
    id: "s1",
    parent: null,
    service: "webui",
    name: "onSubmit",
    kind: "internal",
    node: ID.onSubmit,
    start: 0,
    end: 262,
  },
  {
    id: "s2",
    parent: "s1",
    service: "webui",
    name: "POST /api/orders",
    kind: "client",
    node: ID.createOrder,
    start: 4,
    end: 258,
    attrs: { "http.request.method": "POST", "url.path": "/api/orders" },
  },
  {
    id: "s3",
    parent: "s2",
    service: "api",
    name: "POST /api/orders",
    kind: "server",
    node: ID.routeOrders,
    start: 12,
    end: 250,
    attrs: { "http.route": "/api/orders" },
  },
  {
    id: "s4",
    parent: "s3",
    service: "api",
    name: "orders::create_order",
    kind: "internal",
    node: ID.create_order,
    start: 13,
    end: 249,
    attrs: { "code.function.name": "create_order" },
  },
  {
    id: "s5",
    parent: "s4",
    service: "api",
    name: "Order::validate",
    kind: "internal",
    node: ID.validate,
    start: 14,
    end: 15,
  },
  {
    id: "s6",
    parent: "s4",
    service: "api",
    name: "INSERT orders",
    kind: "client",
    node: ID.insert_order,
    start: 16,
    end: 58,
    attrs: { "db.system": "postgresql", "db.collection.name": "orders" },
  },
  {
    id: "s7",
    parent: "s4",
    service: "api",
    name: "POST /reserve",
    kind: "client",
    node: ID.reserve_stock,
    start: 60,
    end: 232,
  },
  {
    id: "s8",
    parent: "s7",
    service: "worker",
    name: "POST /reserve",
    kind: "server",
    node: ID.routeReserve,
    start: 68,
    end: 226,
    attrs: { "http.route": "/reserve" },
  },
  {
    id: "s9",
    parent: "s8",
    service: "worker",
    name: "reserve",
    kind: "internal",
    node: ID.reserve,
    start: 69,
    end: 225,
    attrs: { "code.function.name": "reserve" },
  },
  {
    id: "s10",
    parent: "s9",
    service: "worker",
    name: "SELECT inventory",
    kind: "client",
    node: ID.check_inventory,
    start: 70,
    end: 102,
    attrs: { "db.collection.name": "inventory" },
  },
  {
    id: "s11",
    parent: "s9",
    service: "worker",
    name: "UPDATE inventory",
    kind: "client",
    node: ID.decrement,
    start: 104,
    end: 140,
    attrs: { "db.collection.name": "inventory" },
  },
  {
    id: "s12",
    parent: "s9",
    service: "worker",
    name: "SELECT inventory",
    kind: "client",
    node: ID.check_inventory,
    start: 142,
    end: 176,
    attrs: { "db.collection.name": "inventory" },
  },
  {
    id: "s13",
    parent: "s9",
    service: "worker",
    name: "UPDATE inventory",
    kind: "client",
    node: ID.decrement,
    start: 178,
    end: 222,
    attrs: { "db.collection.name": "inventory" },
  },
  {
    id: "s14",
    parent: "s4",
    service: "api",
    name: "send_confirmation",
    kind: "producer",
    node: ID.send_confirmation,
    start: 234,
    end: 420,
  },
  {
    id: "s15",
    parent: "s14",
    service: "api",
    name: "smtp send",
    kind: "client",
    node: ID.hostSmtp,
    start: 240,
    end: 415,
    attrs: { "server.address": "smtp.example.com" },
  },
];

export function buildTrace(traceId: string, baseNanos: number, jitter = 1): TraceSummary {
  const spans: RuntimeSpan[] = demoFlowSpans.map((s) => ({
    trace_id: traceId,
    span_id: `${traceId}-${s.id}`,
    parent_span_id: s.parent ? `${traceId}-${s.parent}` : null,
    service: s.service,
    name: s.name,
    kind: s.kind,
    start_unix_nanos: baseNanos + s.start * MS * jitter,
    end_unix_nanos: baseNanos + s.end * MS * jitter,
    error: false,
    attrs: s.attrs ?? {},
    node: s.node,
    join: s.node ? "code_function" : null,
  }));
  const root = spans[0]!;
  return {
    trace_id: traceId,
    root: root.name,
    root_node: root.node,
    start_unix_nanos: root.start_unix_nanos,
    duration_nanos: root.end_unix_nanos - root.start_unix_nanos,
    spans,
  };
}

export function demoStats(scale = 1): NodeStats[] {
  const mk = (
    node: string,
    p50ms: number,
    selfRatio: number,
    rate: number,
    count: number,
  ): NodeStats => {
    const total = p50ms * MS * count;
    return {
      node,
      count,
      errors: node === ID.reserve_stock ? Math.floor(count * 0.01) : 0,
      total_nanos: total,
      self_nanos: Math.floor(total * selfRatio),
      wait_nanos: Math.floor(total * (1 - selfRatio)),
      p50_nanos: p50ms * MS,
      p95_nanos: p50ms * 2.4 * MS,
      p99_nanos: p50ms * 4 * MS,
      max_nanos: p50ms * 7 * MS,
      rate_per_sec: rate * scale,
      last_seen_unix_nanos: Date.now() * MS,
      cpu_nanos: null,
      alloc_bytes: null,
    };
  };
  return [
    mk(ID.onSubmit, 260, 0.02, 0.5, 120),
    mk(ID.createOrder, 250, 0.03, 0.5, 120),
    mk(ID.routeOrders, 238, 0.01, 0.5, 120),
    mk(ID.create_order, 236, 0.05, 0.5, 120),
    mk(ID.validate, 1, 1, 0.5, 120),
    mk(ID.insert_order, 42, 0.1, 0.5, 120),
    mk(ID.reserve_stock, 172, 0.02, 0.5, 120),
    mk(ID.routeReserve, 158, 0.01, 0.5, 120),
    mk(ID.reserve, 156, 0.08, 0.5, 120),
    mk(ID.check_inventory, 33, 0.15, 2.5, 600),
    mk(ID.decrement, 38, 0.12, 1.0, 240),
    mk(ID.send_confirmation, 186, 0.05, 0.5, 120),
    mk(ID.hostSmtp, 175, 1, 0.5, 120),
    mk(ID.fmt_money, 0.01, 1, 40, 9600),
  ];
}

export const demoInsights: Insight[] = [
  {
    kind: "io_in_loop",
    node: ID.reserve,
    effect: "db_read inventory",
    line: 26,
    observed_calls_per_parent: 2.0,
  },
  {
    kind: "waiting",
    node: ID.create_order,
    wait_ratio: 0.93,
    on: [ID.reserve_stock, ID.insert_order],
  },
  {
    kind: "parallelisable",
    node: ID.create_order,
    children: [ID.insert_order, ID.reserve_stock],
    saved_nanos: 42 * MS,
  },
  { kind: "hot_path", node: ID.fmt_money, rate_per_sec: 40, p95_nanos: 0.02 * MS },
  {
    kind: "inefficient",
    node: ID.legacy_format,
    self_p95_nanos: 0,
    complexity: "O(n^2)",
    reasons: ["nested loops over items x fields"],
  },
  { kind: "never_observed", node: ID.nightly },
  { kind: "never_observed", node: ID.legacy_format },
];

export { ID as demoIds };
