import type {
  Edge,
  EntrypointKind,
  Graph,
  GraphNode,
  Insight,
  Language,
  Story,
} from "../src/types";
import { container, edge, fn, synthetic } from "./demo";

interface Part {
  name: string;
  code: string;
  entrypoint?: EntrypointKind;
  effects?: Parameters<typeof fn>[0]["effects"];
  transitive?: Parameters<typeof fn>[0]["transitive"];
  isAsync?: boolean;
}

interface FileSpec {
  path: string;
  lang: Language;
  pkg: string;
  header: string;
  parts: Part[];
}

const files: FileSpec[] = [
  {
    path: "webui/src/lib/components/organisms/spawn/SpawnForm.svelte",
    lang: "typescript",
    pkg: "mod:webui",
    header: `<script lang="ts">
  import { spawnSession } from "$lib/api/sessions";
  import { toast } from "$lib/toast";

  let { machines, accounts, onSpawned } = $props();
  let prompt = $state("");
  let machine = $state(machines[0]?.id ?? "");
  let account = $state(accounts[0]?.id ?? "");
  let busy = $state(false);
`,
    parts: [
      {
        name: "validateSpawn",
        code: `  function validateSpawn(): string | null {
    if (!machine) return "Pick a machine";
    if (!prompt.trim()) return "Write a first prompt";
    return null;
  }`,
      },
      {
        name: "buildSpawnBody",
        code: `  function buildSpawnBody() {
    return { machine_id: machine, account_id: account || null, prompt: prompt.trim(), model: "opus" };
  }`,
      },
      {
        name: "SpawnForm.submit",
        entrypoint: "ui_handler",
        isAsync: true,
        transitive: ["network"],
        code: `  async function submit(event: SubmitEvent) {
    event.preventDefault();
    const problem = validateSpawn();
    if (problem) return toast.error(problem);
    busy = true;
    try {
      const session = await spawnSession(buildSpawnBody());
      onSpawned(session);
    } catch (err) {
      toast.error(\`Spawn failed: \${(err as Error).message}\`);
    } finally {
      busy = false;
    }
  }`,
      },
    ],
  },
  {
    path: "webui/src/lib/api/sessions.ts",
    lang: "typescript",
    pkg: "mod:webui",
    header: `import { request } from "./client";
import type { Session, SpawnBody } from "./types";
`,
    parts: [
      {
        name: "spawnSession",
        isAsync: true,
        effects: [{ kind: "network", target: "POST /api/v1/sessions/spawn", via: "request" }],
        code: `export async function spawnSession(body: SpawnBody): Promise<Session> {
  return request<Session>("POST", "/api/v1/sessions/spawn", body);
}`,
      },
      {
        name: "listSessions",
        isAsync: true,
        effects: [{ kind: "network", target: "GET /api/v1/sessions", via: "request" }],
        code: `export async function listSessions(): Promise<Session[]> {
  return request<Session[]>("GET", "/api/v1/sessions");
}`,
      },
    ],
  },
  {
    path: "webui/src/routes/sessions/SessionList.svelte",
    lang: "typescript",
    pkg: "mod:webui",
    header: `<script lang="ts">
  import { listSessions } from "$lib/api/sessions";
  let sessions = $state([]);
`,
    parts: [
      {
        name: "SessionList.refresh",
        entrypoint: "ui_handler",
        isAsync: true,
        transitive: ["network"],
        code: `  async function refresh() {
    sessions = await listSessions();
  }`,
      },
    ],
  },
  {
    path: "crates/cctui-server-workspace/src/routes/spawn.rs",
    lang: "rust",
    pkg: "mod:crates/cctui-server-workspace",
    header: `use axum::{extract::State, Json};
use cctui_server_core::{accounts, auth::AuthContext, sessions, AppError};

use crate::{daemon_hub::DaemonHub, AppState};
`,
    parts: [
      {
        name: "parse_spawn_request",
        code: `fn parse_spawn_request(body: SpawnBody) -> Result<SpawnRequest, AppError> {
    let prompt = body.prompt.trim().to_owned();
    if prompt.is_empty() {
        return Err(AppError::bad_request("prompt is empty"));
    }
    Ok(SpawnRequest { machine_id: body.machine_id, account_id: body.account_id, prompt, model: body.model })
}`,
      },
      {
        name: "spawn_session",
        entrypoint: "http_handler",
        isAsync: true,
        transitive: ["db_read", "db_write", "network", "spawn"],
        code: `/// POST /api/v1/sessions/spawn: records the session, then asks the machine's daemon to start it.
pub async fn spawn_session(
    State(state): State<AppState>,
    auth: AuthContext,
    Json(body): Json<SpawnBody>,
) -> Result<Json<Session>, AppError> {
    let user = auth.require_user()?;
    let req = parse_spawn_request(body)?;
    let account = accounts::resolve_account(&state.db, user.id, req.account_id).await?;
    let session = sessions::insert_session(&state.db, user.id, &account, &req).await?;
    dispatch_spawn(&state.hub, &state.db, &session, &req).await?;
    Ok(Json(session))
}`,
      },
      {
        name: "dispatch_spawn",
        isAsync: true,
        effects: [
          { kind: "network", target: "daemon websocket", via: "DaemonHub::send" },
          { kind: "spawn", target: "watch_spawn_timeout", via: "tokio::spawn" },
        ],
        transitive: ["sleep", "db_write"],
        code: `async fn dispatch_spawn(hub: &DaemonHub, db: &PgPool, session: &Session, req: &SpawnRequest) -> Result<(), AppError> {
    hub.send(req.machine_id, DaemonMessage::Spawn {
        session_id: session.id,
        prompt: req.prompt.clone(),
        model: req.model.clone(),
    })
    .await?;
    tokio::spawn(watch_spawn_timeout(db.clone(), session.id));
    Ok(())
}`,
      },
      {
        name: "watch_spawn_timeout",
        isAsync: true,
        effects: [
          { kind: "sleep", target: "30s", via: "tokio::time::sleep" },
          { kind: "db_write", target: "sessions", via: "sqlx::query" },
        ],
        code: `async fn watch_spawn_timeout(db: PgPool, id: SessionId) {
    tokio::time::sleep(SPAWN_TIMEOUT).await;
    let _ = sqlx::query("UPDATE sessions SET status = 'failed' WHERE id = $1 AND status = 'starting'")
        .bind(id)
        .execute(&db)
        .await;
}`,
      },
      {
        name: "list_sessions",
        entrypoint: "http_handler",
        isAsync: true,
        effects: [{ kind: "db_read", target: "sessions", via: "sqlx::query_as" }],
        code: `pub async fn list_sessions(State(state): State<AppState>, auth: AuthContext) -> Result<Json<Vec<Session>>, AppError> {
    let user = auth.require_user()?;
    Ok(Json(sessions::for_user(&state.db, user.id).await?))
}`,
      },
    ],
  },
  {
    path: "crates/cctui-server-workspace/src/routes/events.rs",
    lang: "rust",
    pkg: "mod:crates/cctui-server-workspace",
    header: `use axum::{extract::{Path, State}, Json};
`,
    parts: [
      {
        name: "ingest_event",
        entrypoint: "http_handler",
        isAsync: true,
        effects: [{ kind: "db_write", target: "sessions", via: "sqlx::query" }],
        code: `/// POST /api/v1/sessions/{id}/events: daemons report lifecycle changes here.
pub async fn ingest_event(
    State(state): State<AppState>,
    Path(id): Path<SessionId>,
    Json(event): Json<SessionEvent>,
) -> Result<(), AppError> {
    sqlx::query("UPDATE sessions SET status = $2, pid = $3 WHERE id = $1")
        .bind(id)
        .bind(event.status.as_str())
        .bind(event.pid)
        .execute(&state.db)
        .await?;
    state.bus.publish(SessionChanged { id });
    Ok(())
}`,
      },
    ],
  },
  {
    path: "crates/cctui-server-workspace/src/daemon_hub.rs",
    lang: "rust",
    pkg: "mod:crates/cctui-server-workspace",
    header: `use tokio::sync::{mpsc, RwLock};
`,
    parts: [
      {
        name: "DaemonHub::send",
        isAsync: true,
        effects: [
          { kind: "lock", target: "connections", via: "RwLock::read" },
          { kind: "network", target: "daemon websocket", via: "mpsc::Sender::send" },
        ],
        code: `impl DaemonHub {
    pub async fn send(&self, machine: MachineId, msg: DaemonMessage) -> Result<(), AppError> {
        let conns = self.connections.read().await;
        let tx = conns.get(&machine).ok_or_else(|| AppError::conflict("machine offline"))?;
        tx.send(msg).await.map_err(|_| AppError::conflict("daemon disconnected"))
    }
}`,
      },
    ],
  },
  {
    path: "crates/cctui-server-core/src/auth.rs",
    lang: "rust",
    pkg: "mod:crates/cctui-server-core",
    header: "",
    parts: [
      {
        name: "AuthContext::require_user",
        code: `impl AuthContext {
    pub fn require_user(&self) -> Result<&User, AppError> {
        self.user.as_ref().ok_or_else(|| AppError::unauthorized("sign in first"))
    }
}`,
      },
    ],
  },
  {
    path: "crates/cctui-server-core/src/accounts.rs",
    lang: "rust",
    pkg: "mod:crates/cctui-server-core",
    header: "use sqlx::PgPool;\n",
    parts: [
      {
        name: "resolve_account",
        isAsync: true,
        effects: [{ kind: "db_read", target: "accounts", via: "sqlx::query_as" }],
        code: `/// The account to bill: the one asked for, else the user's default.
pub async fn resolve_account(db: &PgPool, user: UserId, wanted: Option<AccountId>) -> Result<Account, AppError> {
    let account = sqlx::query_as::<_, Account>(
        "SELECT * FROM accounts WHERE owner = $1 AND ($2::uuid IS NULL OR id = $2) ORDER BY is_default DESC LIMIT 1",
    )
    .bind(user)
    .bind(wanted)
    .fetch_optional(db)
    .await?;
    account.ok_or_else(|| AppError::bad_request("no account available"))
}`,
      },
    ],
  },
  {
    path: "crates/cctui-server-core/src/sessions.rs",
    lang: "rust",
    pkg: "mod:crates/cctui-server-core",
    header: "use sqlx::PgPool;\n",
    parts: [
      {
        name: "insert_session",
        isAsync: true,
        effects: [{ kind: "db_write", target: "sessions", via: "sqlx::query_as" }],
        code: `pub async fn insert_session(db: &PgPool, owner: UserId, account: &Account, req: &SpawnRequest) -> Result<Session, AppError> {
    Ok(sqlx::query_as::<_, Session>(
        "INSERT INTO sessions (owner, account_id, machine_id, prompt, model, status) \\
         VALUES ($1, $2, $3, $4, $5, 'starting') RETURNING *",
    )
    .bind(owner)
    .bind(account.id)
    .bind(req.machine_id)
    .bind(&req.prompt)
    .bind(&req.model)
    .fetch_one(db)
    .await?)
}`,
      },
      {
        name: "reap_idle_sessions",
        entrypoint: "job",
        isAsync: true,
        effects: [{ kind: "db_write", target: "sessions", via: "sqlx::query" }],
        code: `/// Every minute: archive sessions idle for more than a day.
pub async fn reap_idle_sessions(db: &PgPool) -> Result<u64, AppError> {
    Ok(sqlx::query("UPDATE sessions SET archived = true WHERE last_seen < now() - interval '1 day'")
        .execute(db)
        .await?
        .rows_affected())
}`,
      },
    ],
  },
  {
    path: "crates/cctui-daemon/src/commands/spawn.rs",
    lang: "rust",
    pkg: "mod:crates/cctui-daemon",
    header: `use crate::{adapters::claude::ClaudeAdapter, registry::Registry, report};
`,
    parts: [
      {
        name: "build_env",
        code: `fn build_env(msg: &SpawnMessage) -> Vec<(String, String)> {
    vec![
        ("CCTUI_SESSION_ID".into(), msg.session_id.to_string()),
        ("CLAUDE_MODEL".into(), msg.model.clone()),
    ]
}`,
      },
      {
        name: "handle_spawn",
        entrypoint: "job",
        isAsync: true,
        effects: [{ kind: "lock", target: "registry", via: "Mutex::lock" }],
        transitive: ["process", "spawn", "network"],
        code: `/// Daemon side of DaemonMessage::Spawn: start Claude Code and tell the server it is running.
pub async fn handle_spawn(ctx: &DaemonCtx, msg: SpawnMessage) -> anyhow::Result<()> {
    let slot = ctx.registry.reserve(msg.session_id).await?;
    let child = ClaudeAdapter::launch(&ctx.config, &msg.prompt, build_env(&msg)).await?;
    slot.attach(child.id());
    report::report_spawned(&ctx.http, msg.session_id, child.id()).await?;
    Ok(())
}`,
      },
    ],
  },
  {
    path: "crates/cctui-daemon/src/registry.rs",
    lang: "rust",
    pkg: "mod:crates/cctui-daemon",
    header: "",
    parts: [
      {
        name: "Registry::reserve",
        isAsync: true,
        effects: [{ kind: "lock", target: "registry", via: "Mutex::lock" }],
        code: `impl Registry {
    pub async fn reserve(&self, id: SessionId) -> anyhow::Result<Slot> {
        let mut slots = self.slots.lock().await;
        anyhow::ensure!(!slots.contains_key(&id), "session {id} already running");
        slots.insert(id, SlotState::Starting);
        Ok(Slot { id, registry: self.clone() })
    }
}`,
      },
    ],
  },
  {
    path: "crates/cctui-daemon/src/adapters/claude/mod.rs",
    lang: "rust",
    pkg: "mod:crates/cctui-daemon",
    header: "use tokio::process::Command;\n",
    parts: [
      {
        name: "ClaudeAdapter::launch",
        isAsync: true,
        effects: [
          { kind: "process", target: "claude", via: "tokio::process::Command::spawn" },
          { kind: "spawn", target: "pump_output", via: "tokio::spawn" },
        ],
        code: `impl ClaudeAdapter {
    pub async fn launch(cfg: &Config, prompt: &str, env: Vec<(String, String)>) -> anyhow::Result<Child> {
        let mut child = Command::new(&cfg.claude_bin)
            .args(["--output-format", "stream-json", "--print", prompt])
            .envs(env)
            .stdout(Stdio::piped())
            .spawn()?;
        tokio::spawn(pump_output(child.stdout.take().unwrap()));
        Ok(child)
    }
}`,
      },
    ],
  },
  {
    path: "crates/cctui-daemon/src/report.rs",
    lang: "rust",
    pkg: "mod:crates/cctui-daemon",
    header: "",
    parts: [
      {
        name: "report_spawned",
        isAsync: true,
        effects: [
          { kind: "network", target: "POST /api/v1/sessions/{id}/events", via: "reqwest::Client::post" },
        ],
        code: `pub async fn report_spawned(http: &ServerClient, id: SessionId, pid: Option<u32>) -> anyhow::Result<()> {
    http.post(&format!("/api/v1/sessions/{id}/events"))
        .json(&SessionEvent { status: Status::Running, pid })
        .send()
        .await?
        .error_for_status()?;
    Ok(())
}`,
      },
    ],
  },
  {
    path: "crates/cctui-daemon/src/main.rs",
    lang: "rust",
    pkg: "mod:crates/cctui-daemon",
    header: "",
    parts: [
      {
        name: "main",
        entrypoint: "main",
        isAsync: true,
        code: `#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let ctx = DaemonCtx::from_env().await?;
    ctx.connect_and_serve().await
}`,
      },
    ],
  },
  {
    path: "crates/cctui-admin/src/main.rs",
    lang: "rust",
    pkg: "mod:crates/cctui-admin",
    header: "",
    parts: [
      {
        name: "main",
        entrypoint: "cli",
        code: `fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    args.command.run()
}`,
      },
    ],
  },
  {
    path: "crates/cctui-server-workspace/tests/spawn.rs",
    lang: "rust",
    pkg: "mod:crates/cctui-server-workspace",
    header: "",
    parts: [
      {
        name: "spawn_session_creates_row",
        entrypoint: "test",
        isAsync: true,
        code: `#[tokio::test]
async fn spawn_session_creates_row() {
    let app = TestApp::new().await;
    let res = app.post("/api/v1/sessions/spawn", json!({"machine_id": app.machine, "prompt": "hi"})).await;
    assert_eq!(res.status(), 200);
}`,
      },
      {
        name: "spawn_session_rejects_empty_prompt",
        entrypoint: "test",
        isAsync: true,
        code: `#[tokio::test]
async fn spawn_session_rejects_empty_prompt() {
    let app = TestApp::new().await;
    let res = app.post("/api/v1/sessions/spawn", json!({"machine_id": app.machine, "prompt": " "})).await;
    assert_eq!(res.status(), 400);
}`,
      },
    ],
  },
  {
    path: "webui/journeys/spawn-session.journey.ts",
    lang: "typescript",
    pkg: "mod:webui",
    header: `import { test, expect } from "@playwright/test";
`,
    parts: [
      {
        name: "spawnSessionJourney",
        entrypoint: "test",
        isAsync: true,
        code: `export async function spawnSessionJourney(page) {
  await page.getByRole("button", { name: "Spawn session" }).click();
  await expect(page.getByText("starting")).toBeVisible();
}`,
      },
    ],
  },
];

const PACKAGES: [string, string, Language][] = [
  ["mod:webui", "webui", "typescript"],
  ["mod:crates/cctui-server-workspace", "cctui-server-workspace", "rust"],
  ["mod:crates/cctui-server-core", "cctui-server-core", "rust"],
  ["mod:crates/cctui-daemon", "cctui-daemon", "rust"],
  ["mod:crates/cctui-admin", "cctui-admin", "rust"],
];

const nodes: GraphNode[] = [];
export const cctuiSources: Record<string, string> = {};

for (const [id, name, lang] of PACKAGES) nodes.push(container("package", id, name, name, lang, null, 0));
for (const f of files) {
  let text = f.header;
  const fileNodes: GraphNode[] = [];
  for (const part of f.parts) {
    if (text && !text.endsWith("\n\n")) text += "\n";
    const start = text.split("\n").length;
    text += `${part.code}\n`;
    const end = start + part.code.split("\n").length - 1;
    const first = part.code.split("\n").find((l) => /fn |function /.test(l)) ?? part.code;
    fileNodes.push(
      fn({
        file: f.path,
        name: part.name,
        lang: f.lang,
        lines: [start, end],
        signature: first.trim().replace(/\s*\{$/, ""),
        isAsync: part.isAsync,
        effects: part.effects,
        transitive: part.transitive,
        entrypoint: part.entrypoint,
      }),
    );
  }
  if (f.path.endsWith(".svelte")) text += "</script>\n";
  cctuiSources[f.path] = text;
  const lines = text.split("\n").length;
  nodes.push(
    container("file", `file:${f.path}`, f.path.split("/").pop()!, f.path, f.lang, f.pkg, lines, f.path),
    ...fileNodes,
  );
}

const T = {
  accounts: "table:accounts",
  sessions: "table:sessions",
  anthropic: "host:api.anthropic.com",
  claude: "ext:claude",
  routeSpawn: "route:POST /api/v1/sessions/spawn",
  routeEvents: "route:POST /api/v1/sessions/{}/events",
  routeList: "route:GET /api/v1/sessions",
};
nodes.push(
  synthetic("table", T.accounts, "accounts"),
  synthetic("table", T.sessions, "sessions"),
  synthetic("host", T.anthropic, "api.anthropic.com"),
  synthetic("dependency", T.claude, "claude"),
  synthetic("route", T.routeSpawn, "POST /api/v1/sessions/spawn"),
  synthetic("route", T.routeEvents, "POST /api/v1/sessions/{}/events"),
  synthetic("route", T.routeList, "GET /api/v1/sessions"),
);

const W = "webui/src/lib/components/organisms/spawn/SpawnForm.svelte";
const S = "crates/cctui-server-workspace/src/routes/spawn.rs";
const D = "crates/cctui-daemon/src";
export const cctuiIds = {
  submit: `fn:${W}#SpawnForm.submit`,
  validateSpawn: `fn:${W}#validateSpawn`,
  buildSpawnBody: `fn:${W}#buildSpawnBody`,
  spawnSession: "fn:webui/src/lib/api/sessions.ts#spawnSession",
  listSessions: "fn:webui/src/lib/api/sessions.ts#listSessions",
  refresh: "fn:webui/src/routes/sessions/SessionList.svelte#SessionList.refresh",
  spawn_session: `fn:${S}#spawn_session`,
  parse_spawn_request: `fn:${S}#parse_spawn_request`,
  dispatch_spawn: `fn:${S}#dispatch_spawn`,
  watch_spawn_timeout: `fn:${S}#watch_spawn_timeout`,
  list_sessions: `fn:${S}#list_sessions`,
  require_user: "fn:crates/cctui-server-core/src/auth.rs#AuthContext::require_user",
  resolve_account: "fn:crates/cctui-server-core/src/accounts.rs#resolve_account",
  insert_session: "fn:crates/cctui-server-core/src/sessions.rs#insert_session",
  reap: "fn:crates/cctui-server-core/src/sessions.rs#reap_idle_sessions",
  hub_send: "fn:crates/cctui-server-workspace/src/daemon_hub.rs#DaemonHub::send",
  ingest_event: "fn:crates/cctui-server-workspace/src/routes/events.rs#ingest_event",
  handle_spawn: `fn:${D}/commands/spawn.rs#handle_spawn`,
  build_env: `fn:${D}/commands/spawn.rs#build_env`,
  reserve: `fn:${D}/registry.rs#Registry::reserve`,
  launch: `fn:${D}/adapters/claude/mod.rs#ClaudeAdapter::launch`,
  report_spawned: `fn:${D}/report.rs#report_spawned`,
  test_row: "fn:crates/cctui-server-workspace/tests/spawn.rs#spawn_session_creates_row",
  ...T,
};
const I = cctuiIds;

const contains: Edge[] = nodes.filter((n) => n.parent).map((n) => edge(n.parent!, n.id, "contains"));
const edges: Edge[] = [
  ...contains,
  edge(I.submit, I.validateSpawn, "calls", "exact", { line: 22 }),
  edge(I.submit, I.spawnSession, "calls", "exact", { line: 26, awaited: true }),
  edge(I.submit, I.buildSpawnBody, "calls", "exact", { line: 26 }),
  edge(I.spawnSession, I.routeSpawn, "requests", "heuristic", { line: 5 }),
  edge(I.refresh, I.listSessions, "calls", "exact", { awaited: true }),
  edge(I.listSessions, I.routeList, "requests", "heuristic"),
  edge(I.routeList, I.list_sessions, "handles"),
  edge(I.list_sessions, I.require_user, "calls"),
  edge(I.list_sessions, I.sessions, "reads", "heuristic"),
  edge(I.routeSpawn, I.spawn_session, "handles"),
  edge(I.spawn_session, I.require_user, "calls", "exact", { line: 20 }),
  edge(I.spawn_session, I.parse_spawn_request, "calls", "exact", { line: 21 }),
  edge(I.spawn_session, I.resolve_account, "calls", "exact", { line: 22, awaited: true }),
  edge(I.spawn_session, I.insert_session, "calls", "exact", { line: 23, awaited: true }),
  edge(I.spawn_session, I.dispatch_spawn, "calls", "exact", { line: 24, awaited: true }),
  edge(I.resolve_account, I.accounts, "reads", "heuristic"),
  edge(I.insert_session, I.sessions, "writes", "heuristic"),
  edge(I.dispatch_spawn, I.hub_send, "calls", "exact", { line: 29, awaited: true }),
  edge(I.dispatch_spawn, I.watch_spawn_timeout, "spawns", "exact", { line: 35 }),
  edge(I.hub_send, I.handle_spawn, "requests", "heuristic"),
  edge(I.watch_spawn_timeout, I.sessions, "writes", "heuristic"),
  edge(I.handle_spawn, I.reserve, "calls", "exact", { awaited: true }),
  edge(I.handle_spawn, I.build_env, "calls", "exact"),
  edge(I.handle_spawn, I.launch, "calls", "exact", { awaited: true }),
  edge(I.handle_spawn, I.report_spawned, "calls", "exact", { awaited: true }),
  edge(I.launch, I.claude, "spawns", "heuristic"),
  edge(I.claude, I.anthropic, "requests", "heuristic"),
  edge(I.report_spawned, I.routeEvents, "requests", "heuristic"),
  edge(I.routeEvents, I.ingest_event, "handles"),
  edge(I.ingest_event, I.sessions, "writes", "heuristic"),
  edge(I.reap, I.sessions, "writes", "heuristic"),
  edge(I.test_row, I.spawn_session, "calls", "exact", { awaited: true }),
];

export const cctuiGraph: Graph = {
  meta: {
    version: 1,
    root: "/home/dev/cctui",
    generated_at: "2026-10-31T08:17:31Z",
    languages: ["rust", "typescript"],
    files: files.length,
    warnings: [],
  },
  nodes,
  edges,
};

const node = (id: string) => nodes.find((n) => n.id === id)!;
const fx = (id: string) => node(id).effects.direct;

/** What `/api/story?from=SpawnForm.submit` returns for cctui: the click on "Spawn session" end to end. */
export const cctuiSpawnStory: Story = {
  root: I.submit,
  lanes: [
    { id: "webui", label: "Web UI", kind: "ui" },
    { id: "server", label: "cctui-server", kind: "service" },
    { id: "daemon", label: "cctui-daemon", kind: "worker" },
    { id: "postgres", label: "Postgres", kind: "database" },
    { id: "external", label: "External", kind: "external" },
  ],
  steps: [
    { node: I.submit, lane: "webui", label: "SpawnForm.submit", kind: "function", seq: 0, effects: [], folded: [I.validateSpawn, I.buildSpawnBody] },
    { node: I.spawnSession, lane: "webui", label: "api.spawnSession", kind: "function", seq: 1, effects: fx(I.spawnSession), folded: [] },
    { node: I.routeSpawn, lane: "server", label: "POST /api/v1/sessions/spawn", kind: "route", seq: 2, effects: [], folded: [] },
    { node: I.spawn_session, lane: "server", label: "spawn_session", kind: "function", seq: 3, effects: [], folded: [I.require_user, I.parse_spawn_request] },
    { node: I.resolve_account, lane: "server", label: "accounts::resolve_account", kind: "function", seq: 4, effects: fx(I.resolve_account), folded: [] },
    { node: I.accounts, lane: "postgres", label: "accounts", kind: "table", seq: 5, effects: [], folded: [] },
    { node: I.insert_session, lane: "server", label: "sessions::insert_session", kind: "function", seq: 6, effects: fx(I.insert_session), folded: [] },
    { node: I.sessions, lane: "postgres", label: "sessions", kind: "table", seq: 7, effects: [], folded: [] },
    { node: I.dispatch_spawn, lane: "server", label: "dispatch_spawn", kind: "function", seq: 8, effects: [...fx(I.dispatch_spawn), ...fx(I.hub_send)], folded: [I.hub_send] },
    { node: I.watch_spawn_timeout, lane: "server", label: "watch_spawn_timeout", kind: "function", seq: 9, effects: fx(I.watch_spawn_timeout), folded: [] },
    { node: I.handle_spawn, lane: "daemon", label: "handle_spawn", kind: "function", seq: 10, effects: fx(I.handle_spawn), folded: [I.reserve, I.build_env] },
    { node: I.launch, lane: "daemon", label: "ClaudeAdapter::launch", kind: "function", seq: 11, effects: fx(I.launch), folded: [] },
    { node: I.claude, lane: "external", label: "claude (process)", kind: "dependency", seq: 12, effects: [], folded: [] },
    { node: I.anthropic, lane: "external", label: "api.anthropic.com", kind: "host", seq: 13, effects: [], folded: [] },
    { node: I.report_spawned, lane: "daemon", label: "report_spawned", kind: "function", seq: 14, effects: fx(I.report_spawned), folded: [] },
    { node: I.routeEvents, lane: "server", label: "POST /api/v1/sessions/{id}/events", kind: "route", seq: 15, effects: [], folded: [] },
    { node: I.ingest_event, lane: "server", label: "ingest_event", kind: "function", seq: 16, effects: fx(I.ingest_event), folded: [] },
  ],
  links: [
    { from: I.submit, to: I.spawnSession, kind: "calls", label: null, seq: 1, detached: false, confidence: "exact" },
    { from: I.spawnSession, to: I.routeSpawn, kind: "requests", label: "fetch POST", seq: 2, detached: false, confidence: "heuristic" },
    { from: I.routeSpawn, to: I.spawn_session, kind: "handles", label: null, seq: 3, detached: false, confidence: "exact" },
    { from: I.spawn_session, to: I.resolve_account, kind: "calls", label: null, seq: 4, detached: false, confidence: "exact" },
    { from: I.resolve_account, to: I.accounts, kind: "reads", label: "SELECT accounts", seq: 5, detached: false, confidence: "heuristic" },
    { from: I.spawn_session, to: I.insert_session, kind: "calls", label: null, seq: 6, detached: false, confidence: "exact" },
    { from: I.insert_session, to: I.sessions, kind: "writes", label: "INSERT sessions", seq: 7, detached: false, confidence: "heuristic" },
    { from: I.spawn_session, to: I.dispatch_spawn, kind: "calls", label: null, seq: 8, detached: false, confidence: "exact" },
    { from: I.dispatch_spawn, to: I.watch_spawn_timeout, kind: "spawns", label: "tokio::spawn", seq: 9, detached: true, confidence: "exact" },
    { from: I.watch_spawn_timeout, to: I.sessions, kind: "writes", label: "UPDATE on timeout", seq: 9, detached: false, confidence: "heuristic" },
    { from: I.dispatch_spawn, to: I.handle_spawn, kind: "requests", label: "Spawn msg · WebSocket", seq: 10, detached: true, confidence: "heuristic" },
    { from: I.handle_spawn, to: I.launch, kind: "calls", label: null, seq: 11, detached: false, confidence: "exact" },
    { from: I.launch, to: I.claude, kind: "spawns", label: "spawn process", seq: 12, detached: true, confidence: "heuristic" },
    { from: I.claude, to: I.anthropic, kind: "requests", label: "HTTPS", seq: 13, detached: false, confidence: "heuristic" },
    { from: I.handle_spawn, to: I.report_spawned, kind: "calls", label: null, seq: 14, detached: false, confidence: "exact" },
    { from: I.report_spawned, to: I.routeEvents, kind: "requests", label: "POST status=running", seq: 15, detached: false, confidence: "heuristic" },
    { from: I.routeEvents, to: I.ingest_event, kind: "handles", label: null, seq: 16, detached: false, confidence: "exact" },
    { from: I.ingest_event, to: I.sessions, kind: "writes", label: "UPDATE sessions", seq: 17, detached: false, confidence: "heuristic" },
  ],
  truncated: false,
};

export const cctuiInsights: Insight[] = [
  { kind: "io_in_loop", node: I.test_row, effect: "network", line: 4, observed_calls_per_parent: null },
  { kind: "never_observed", node: I.reap },
  { kind: "waiting", node: I.spawn_session, wait_ratio: 0.82, on: [I.resolve_account, I.insert_session] },
  { kind: "parallelisable", node: I.spawn_session, children: [I.resolve_account, I.insert_session], saved_nanos: 4_200_000 },
];
