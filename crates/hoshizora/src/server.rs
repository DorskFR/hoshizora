//! HTTP API from `docs/api.md`, the OTLP/HTTP receiver and the embedded viewer.

use std::collections::BTreeMap;
use std::convert::Infallible;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::Bytes;
use axum::extract::{FromRequestParts, Query, RawPathParams, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, Method, StatusCode, Uri, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use futures::Stream;
use hoshizora_model::api::{
    AddProject, Insights, NodeDetail, Project, ProjectStatus, PurgeReport, Purged, Source,
    StatesReport, Storage, UpdateProject,
};
use hoshizora_model::{EdgeKind, Graph, Insight, LiveEvent, Node, NodeStats, RuntimeSpan};
use hoshizora_runtime::route::{ProjectCandidate, project_for_span};
use hoshizora_runtime::{NodeLookup, Runtime, RuntimeConfig, StatsSnapshot, Trace, otlp};
use serde::Deserialize;
use tokio::sync::broadcast;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;
use tower_http::compression::CompressionLayer;
use tower_http::cors::{AllowHeaders, AllowOrigin, CorsLayer};

use crate::assets;
pub use crate::projects::AppState;
use crate::queries::{GraphQueries, parse_edge_kinds, parse_effect_kind};

/// One ready project: its graph, runtime and live event stream.
pub struct ProjectState<Q: GraphQueries> {
    pub queries: Arc<Q>,
    pub runtime: Mutex<Runtime<Arc<Q>>>,
    pub events: broadcast::Sender<LiveEvent>,
    pub root: PathBuf,
    /// `/api/graph` bodies, serialised once: the graph is immutable until a reindex replaces
    /// the whole `ProjectState`.
    graph_json: Bytes,
    slim_graph_json: Bytes,
    stats_restored: AtomicBool,
}

impl<Q: GraphQueries + 'static> ProjectState<Q> {
    pub fn new(queries: Q, root: PathBuf, config: RuntimeConfig) -> Arc<Self> {
        let queries = Arc::new(queries);
        let runtime = Runtime::new(config, Arc::clone(&queries));
        let (events, _) = broadcast::channel(4096);
        let graph_json = Bytes::from(serde_json::to_vec(queries.graph()).unwrap_or_default());
        let slim_graph_json =
            Bytes::from(serde_json::to_vec(&slim_graph(queries.graph())).unwrap_or_default());
        Arc::new(Self {
            queries,
            runtime: Mutex::new(runtime),
            events,
            root,
            graph_json,
            slim_graph_json,
            stats_restored: AtomicBool::new(false),
        })
    }

    fn runtime(&self) -> std::sync::MutexGuard<'_, Runtime<Arc<Q>>> {
        self.runtime.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Broadcasts stats for nodes that changed since the previous tick.
    pub fn publish_stats(&self) {
        let now = hoshizora_runtime::now_unix_nanos();
        let changed = self.runtime().take_changed(now);
        if !changed.is_empty() {
            let _ = self.events.send(LiveEvent::Stats { stats: changed });
        }
    }

    /// Merges the aggregates saved at `path` into the runtime, once per `ProjectState`.
    pub fn restore_stats(&self, path: &Path) {
        if self.stats_restored.swap(true, Ordering::SeqCst) {
            return;
        }
        if let Some(snapshot) = StatsSnapshot::load(path) {
            let added = self.runtime().import_stats(&snapshot);
            tracing::info!(path = %path.display(), nodes = added, "restored runtime stats");
        }
    }

    /// Writes the aggregates to `path`; nothing is written while there are none.
    pub fn persist_stats(&self, path: &Path) {
        let snapshot = self
            .runtime()
            .export_stats(hoshizora_runtime::now_unix_nanos());
        if snapshot.nodes.is_empty() {
            return;
        }
        if let Err(e) = snapshot.save(path) {
            tracing::warn!(path = %path.display(), error = %e, "saving runtime stats");
        }
    }

    fn stats_persist_secs(&self) -> u64 {
        self.runtime().config().stats_persist_secs.max(1)
    }
}

/// `<data_dir>/stats/<project>.json`.
pub fn stats_path(data_dir: &Path, project: &str) -> PathBuf {
    data_dir.join("stats").join(format!("{project}.json"))
}

/// Saves every ready project's aggregates.
pub fn persist_all_stats<Q: GraphQueries + 'static>(state: &AppState<Q>) {
    for (id, _, project) in state.candidates() {
        project.persist_stats(&stats_path(&state.data_dir, &id));
    }
}

/// The project a request addresses: `{project}` in the path, else the first registered one.
pub struct Proj<Q: GraphQueries>(pub Arc<ProjectState<Q>>);

impl<Q: GraphQueries> FromRequestParts<Arc<AppState<Q>>> for Proj<Q> {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState<Q>>,
    ) -> Result<Self, AppError> {
        let params = RawPathParams::from_request_parts(parts, state)
            .await
            .map_err(|e| AppError::bad_request(e.to_string()))?;
        let id = params
            .iter()
            .find(|(k, _)| *k == "project")
            .map(|(_, v)| v.to_string());
        state.project_state(id.as_deref()).map(Proj)
    }
}

fn path_param(params: &RawPathParams, name: &str) -> Result<String, AppError> {
    params
        .iter()
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v.to_string())
        .ok_or_else(|| AppError::bad_request(format!("missing {name}")))
}

pub fn router<Q: GraphQueries + 'static>(state: Arc<AppState<Q>>) -> Router {
    let api = project_routes::<Q>("/api")
        .merge(project_routes::<Q>("/api/projects/{project}"))
        .route(
            "/api/projects",
            get(list_projects::<Q>).post(add_project::<Q>),
        )
        .route(
            "/api/projects/{project}",
            get(get_project::<Q>)
                .patch(update_project::<Q>)
                .delete(delete_project::<Q>),
        )
        .route(
            "/api/projects/{project}/reindex",
            post(reindex_project::<Q>),
        )
        .route("/api/projects/{project}/purge", post(purge_project::<Q>))
        .route("/api/purge", post(purge_all::<Q>))
        .route("/api/storage", get(storage::<Q>))
        .layer(CompressionLayer::new());
    Router::new()
        .merge(api)
        .route("/api/runtime/stream", get(stream::<Q>))
        .route("/api/projects/{project}/runtime/stream", get(stream::<Q>))
        .route("/v1/traces", post(traces_ingest::<Q>))
        .fallback(static_assets)
        .layer(cors())
        .with_state(state)
}

/// Every graph/query/runtime route under `prefix`, resolved against that project's graph.
fn project_routes<Q: GraphQueries + 'static>(prefix: &str) -> Router<Arc<AppState<Q>>> {
    let p = |s: &str| format!("{prefix}{s}");
    Router::new()
        .route(&p("/graph"), get(graph::<Q>))
        .route(&p("/nodes/{*id}"), get(node_detail::<Q>))
        .route(&p("/source"), get(source::<Q>))
        .route(&p("/search"), get(search::<Q>))
        .route(&p("/entrypoints"), get(entrypoints::<Q>))
        .route(&p("/flow"), get(flow::<Q>))
        .route(&p("/impact"), get(impact::<Q>))
        .route(&p("/story"), get(story::<Q>))
        .route(&p("/dead"), get(dead::<Q>))
        .route(&p("/effects"), get(effects::<Q>))
        .route(&p("/insights"), get(insights::<Q>))
        .route(&p("/states"), get(states::<Q>))
        .route(&p("/runtime/stats"), get(runtime_stats::<Q>))
        .route(&p("/runtime/traces"), get(runtime_traces::<Q>))
        .route(&p("/runtime/traces/{trace_id}"), get(runtime_trace::<Q>))
}

/// Browser OTel exporters send credentials, which a wildcard origin would make the browser reject.
fn cors() -> CorsLayer {
    CorsLayer::new()
        .allow_origin(AllowOrigin::mirror_request())
        .allow_credentials(true)
        .allow_headers(AllowHeaders::mirror_request())
        .allow_methods([Method::GET, Method::POST, Method::DELETE, Method::OPTIONS])
        .max_age(Duration::from_secs(3600))
}

/// Every second while the server runs: restores saved aggregates into projects that have not
/// loaded them yet, emits a `Stats` live event per project, and periodically saves aggregates.
pub fn spawn_stats_ticker<Q: GraphQueries + 'static>(state: Arc<AppState<Q>>) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        let mut ticks: u64 = 0;
        loop {
            tick.tick().await;
            ticks += 1;
            for (id, _, project) in state.candidates() {
                let path = stats_path(&state.data_dir, &id);
                project.restore_stats(&path);
                project.publish_stats();
                if ticks.is_multiple_of(project.stats_persist_secs()) {
                    project.persist_stats(&path);
                }
            }
        }
    });
}

/// JSON error body `{ "error": ..., "status"?: project status }`.
#[derive(Debug)]
pub struct AppError {
    pub status: StatusCode,
    pub message: String,
    pub project_status: Option<ProjectStatus>,
}

impl AppError {
    pub fn not_found(what: &str) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: format!("{what} not found"),
            project_status: None,
        }
    }

    pub fn bad_request(msg: impl ToString) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: msg.to_string(),
            project_status: None,
        }
    }

    pub fn not_ready(status: ProjectStatus, msg: &str) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            message: msg.to_string(),
            project_status: Some(status),
        }
    }

    pub fn gone(msg: impl ToString) -> Self {
        Self {
            status: StatusCode::GONE,
            message: msg.to_string(),
            project_status: None,
        }
    }

    pub fn internal(msg: impl ToString) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: msg.to_string(),
            project_status: None,
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let mut body = serde_json::json!({ "error": self.message });
        if let Some(status) = self.project_status {
            body["status"] = serde_json::to_value(status).unwrap_or_default();
        }
        (self.status, Json(body)).into_response()
    }
}

fn not_found(what: &str) -> AppError {
    AppError::not_found(what)
}

fn bad_request(msg: impl ToString) -> AppError {
    AppError::bad_request(msg)
}

async fn list_projects<Q: GraphQueries>(
    State(state): State<Arc<AppState<Q>>>,
) -> Json<Vec<Project>> {
    Json(state.list())
}

async fn add_project<Q: GraphQueries>(
    State(state): State<Arc<AppState<Q>>>,
    Json(req): Json<AddProject>,
) -> Result<(StatusCode, Json<Project>), AppError> {
    state.add_git(req).map(|p| (StatusCode::ACCEPTED, Json(p)))
}

async fn get_project<Q: GraphQueries>(
    State(state): State<Arc<AppState<Q>>>,
    params: RawPathParams,
) -> Result<Json<Project>, AppError> {
    let id = path_param(&params, "project")?;
    state.get(&id).map(Json).ok_or_else(|| not_found("project"))
}

async fn update_project<Q: GraphQueries>(
    State(state): State<Arc<AppState<Q>>>,
    params: RawPathParams,
    Json(req): Json<UpdateProject>,
) -> Result<Json<Project>, AppError> {
    let id = path_param(&params, "project")?;
    state.update(&id, req).map(Json)
}

async fn delete_project<Q: GraphQueries>(
    State(state): State<Arc<AppState<Q>>>,
    params: RawPathParams,
) -> Result<StatusCode, AppError> {
    let id = path_param(&params, "project")?;
    state.remove(&id).map(|()| StatusCode::NO_CONTENT)
}

async fn reindex_project<Q: GraphQueries>(
    State(state): State<Arc<AppState<Q>>>,
    params: RawPathParams,
) -> Result<(StatusCode, Json<Project>), AppError> {
    let id = path_param(&params, "project")?;
    state.reindex(&id).map(|p| (StatusCode::ACCEPTED, Json(p)))
}

async fn purge_project<Q: GraphQueries + 'static>(
    State(state): State<Arc<AppState<Q>>>,
    params: RawPathParams,
) -> Result<Json<Purged>, AppError> {
    let id = path_param(&params, "project")?;
    tokio::task::spawn_blocking(move || state.purge(&id))
        .await
        .map_err(|e| AppError::internal(format!("purge task panicked: {e}")))?
        .map(Json)
}

async fn purge_all<Q: GraphQueries + 'static>(
    State(state): State<Arc<AppState<Q>>>,
) -> Result<Json<PurgeReport>, AppError> {
    tokio::task::spawn_blocking(move || state.purge_all())
        .await
        .map(Json)
        .map_err(|e| AppError::internal(format!("purge task panicked: {e}")))
}

async fn storage<Q: GraphQueries + 'static>(
    State(state): State<Arc<AppState<Q>>>,
) -> Result<Json<Storage>, AppError> {
    tokio::task::spawn_blocking(move || state.storage())
        .await
        .map(Json)
        .map_err(|e| AppError::internal(format!("storage task panicked: {e}")))
}

#[derive(Deserialize)]
struct GraphParams {
    detail: Option<String>,
}

async fn graph<Q: GraphQueries>(Proj(state): Proj<Q>, Query(p): Query<GraphParams>) -> Response {
    let body = match p.detail.as_deref() {
        Some("slim") => state.slim_graph_json.clone(),
        _ => state.graph_json.clone(),
    };
    ([(header::CONTENT_TYPE, "application/json")], body).into_response()
}

/// Same `Graph` shape with the fields the 3D view never reads stripped: docs, signatures,
/// direct effects, edge attrs (except `candidates`) and structural `contains`/`imports`/`uses`
/// edges, which the viewer rebuilds from `Node.parent`.
pub fn slim_graph(graph: &Graph) -> Graph {
    let nodes = graph
        .nodes
        .iter()
        .map(|n| {
            let mut n = n.clone();
            if let Some(f) = n.function.as_mut() {
                f.doc = None;
                f.signature = String::new();
                f.params = Vec::new();
                f.attributes = Vec::new();
                f.complexity.basis = Vec::new();
            }
            n
        })
        .collect();
    let edges = graph
        .edges
        .iter()
        .filter(|e| {
            !matches!(
                e.kind,
                EdgeKind::Contains | EdgeKind::Imports | EdgeKind::Uses
            )
        })
        .map(|e| {
            let mut e = e.clone();
            e.attrs.retain(|k, _| k == "candidates");
            e
        })
        .collect();
    Graph {
        meta: graph.meta.clone(),
        nodes,
        edges,
        states: hoshizora_model::States::default(),
    }
}

async fn node_detail<Q: GraphQueries>(
    Proj(state): Proj<Q>,
    params: RawPathParams,
) -> Result<Json<NodeDetail>, AppError> {
    let id = decode_id(&path_param(&params, "id")?);
    let node = state
        .queries
        .node(&id)
        .ok_or_else(|| not_found("node"))?
        .clone();
    let source = match (&node.file, node.span) {
        (Some(file), Some(span)) => {
            read_source(&state.root, file, span.start_line, span.end_line).ok()
        }
        _ => None,
    };
    let stats = state
        .runtime()
        .node_stats(&id, hoshizora_runtime::now_unix_nanos());
    Ok(Json(NodeDetail {
        incoming: state.queries.incoming(&id),
        outgoing: state.queries.outgoing(&id),
        node,
        source,
        stats,
    }))
}

fn decode_id(raw: &str) -> String {
    if raw.contains('%') {
        percent_encoding::percent_decode_str(raw)
            .decode_utf8_lossy()
            .into_owned()
    } else {
        raw.to_string()
    }
}

#[derive(Deserialize)]
struct SourceParams {
    file: String,
    start: Option<u32>,
    end: Option<u32>,
}

async fn source<Q: GraphQueries>(
    Proj(state): Proj<Q>,
    Query(p): Query<SourceParams>,
) -> Result<Json<Source>, AppError> {
    let start = p.start.unwrap_or(1).max(1);
    let end = p.end.unwrap_or(u32::MAX).max(start);
    if !state.root.exists() {
        return Err(AppError::gone(SOURCE_UNAVAILABLE));
    }
    read_source(&state.root, &p.file, start, end).map(Json)
}

pub const SOURCE_UNAVAILABLE: &str =
    "source not available: the project's checkout is missing (purged clone); reindex to restore it";

/// Reads lines `start..=end` (1-based) of `file`, which must stay inside `root`.
pub fn read_source(root: &Path, file: &str, start: u32, end: u32) -> Result<Source, AppError> {
    let rel = Path::new(file);
    if rel.is_absolute()
        || rel
            .components()
            .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
    {
        return Err(bad_request(
            "file must be a relative path inside the indexed root",
        ));
    }
    let root_canon = root.canonicalize().map_err(|_| not_found("root"))?;
    let full = root_canon
        .join(rel)
        .canonicalize()
        .map_err(|_| not_found("file"))?;
    if !full.starts_with(&root_canon) {
        return Err(bad_request("file escapes the indexed root"));
    }
    let text = std::fs::read_to_string(&full).map_err(|_| not_found("file"))?;
    let lines: Vec<&str> = text.lines().collect();
    let total = u32::try_from(lines.len()).unwrap_or(u32::MAX);
    let end = end.min(total);
    let selected = if start > end {
        String::new()
    } else {
        lines[(start - 1) as usize..end as usize].join("\n")
    };
    Ok(Source {
        file: file.to_string(),
        start_line: start,
        end_line: end,
        text: selected,
    })
}

#[derive(Deserialize)]
struct SearchParams {
    q: String,
    limit: Option<usize>,
}

async fn search<Q: GraphQueries>(Proj(state): Proj<Q>, Query(p): Query<SearchParams>) -> Response {
    Json(state.queries.search(&p.q, p.limit.unwrap_or(20))).into_response()
}

async fn entrypoints<Q: GraphQueries>(Proj(state): Proj<Q>) -> Json<Vec<Node>> {
    Json(state.queries.entrypoints())
}

#[derive(Deserialize)]
struct FlowParams {
    from: String,
    depth: Option<u32>,
    kinds: Option<String>,
}

pub const DEFAULT_DEPTH: u32 = 8;

async fn flow<Q: GraphQueries>(
    Proj(state): Proj<Q>,
    Query(p): Query<FlowParams>,
) -> Result<Response, AppError> {
    let kinds = parse_edge_kinds(p.kinds.as_deref().unwrap_or_default())
        .map_err(|e| bad_request(e.to_string()))?;
    state
        .queries
        .flow(&p.from, p.depth.unwrap_or(DEFAULT_DEPTH), &kinds)
        .map(|s| Json(s).into_response())
        .ok_or_else(|| not_found("node"))
}

#[derive(Deserialize)]
struct ImpactParams {
    of: String,
    depth: Option<u32>,
}

async fn impact<Q: GraphQueries>(
    Proj(state): Proj<Q>,
    Query(p): Query<ImpactParams>,
) -> Result<Response, AppError> {
    state
        .queries
        .impact(&p.of, p.depth.unwrap_or(DEFAULT_DEPTH))
        .map(|s| Json(s).into_response())
        .ok_or_else(|| not_found("node"))
}

#[derive(Deserialize)]
struct StoryParams {
    from: String,
    depth: Option<u32>,
}

async fn story<Q: GraphQueries>(
    Proj(state): Proj<Q>,
    Query(p): Query<StoryParams>,
) -> Result<Response, AppError> {
    state
        .queries
        .story(&p.from, p.depth.unwrap_or(DEFAULT_DEPTH))
        .map(|s| Json(s).into_response())
        .ok_or_else(|| not_found("node"))
}

async fn dead<Q: GraphQueries>(Proj(state): Proj<Q>) -> Json<Vec<Node>> {
    Json(state.queries.dead())
}

#[derive(Deserialize)]
struct EffectsParams {
    kind: String,
    target: Option<String>,
}

async fn effects<Q: GraphQueries>(
    Proj(state): Proj<Q>,
    Query(p): Query<EffectsParams>,
) -> Result<Json<Vec<Node>>, AppError> {
    let kind = parse_effect_kind(&p.kind).map_err(|e| bad_request(e.to_string()))?;
    Ok(Json(state.queries.with_effect(kind, p.target.as_deref())))
}

async fn insights<Q: GraphQueries>(Proj(state): Proj<Q>) -> Json<Insights> {
    Json(Insights {
        insights: all_insights(&state),
    })
}

#[derive(Deserialize)]
struct StatesParams {
    domain: Option<String>,
}

async fn states<Q: GraphQueries>(
    Proj(state): Proj<Q>,
    Query(p): Query<StatesParams>,
) -> Json<StatesReport> {
    Json(state.queries.states(p.domain.as_deref()))
}

pub fn all_insights<Q: GraphQueries>(state: &ProjectState<Q>) -> Vec<Insight> {
    let statics = state.queries.static_insights();
    state
        .runtime()
        .insights(hoshizora_runtime::now_unix_nanos(), &statics)
}

struct Candidate<Q: GraphQueries> {
    id: String,
    services: Vec<String>,
    state: Arc<ProjectState<Q>>,
}

impl<Q: GraphQueries> ProjectCandidate for Candidate<Q> {
    fn id(&self) -> &str {
        &self.id
    }
    fn services(&self) -> &[String] {
        &self.services
    }
    fn locates(&self, span: &RuntimeSpan) -> bool {
        self.state.runtime().locates(span)
    }
}

#[derive(Deserialize)]
struct IngestParams {
    project: Option<String>,
}

async fn traces_ingest<Q: GraphQueries>(
    State(state): State<Arc<AppState<Q>>>,
    Query(q): Query<IngestParams>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or(otlp::CONTENT_TYPE_PROTOBUF)
        .to_string();
    let encoding = headers
        .get(header::CONTENT_ENCODING)
        .and_then(|v| v.to_str().ok());
    let spans = match otlp::decode(&content_type, encoding, &body) {
        Ok(spans) => spans,
        Err(e) => {
            tracing::warn!(error = %e, "rejected OTLP export");
            return AppError::bad_request(e).into_response();
        }
    };
    let hint = headers
        .get("x-hoshizora-project")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
        .or(q.project);
    let candidates: Vec<Candidate<Q>> = state
        .candidates()
        .into_iter()
        .map(|(id, services, state)| Candidate {
            id,
            services,
            state,
        })
        .collect();
    let count = spans.len();
    let mut per_project: Vec<Vec<RuntimeSpan>> = vec![Vec::new(); candidates.len()];
    let mut unplaced = 0usize;
    for span in spans {
        match project_for_span(&span, hint.as_deref(), &candidates) {
            Ok(Some(i)) => per_project[i].push(span),
            Ok(None) => unplaced += 1,
            Err(e) => return AppError::not_found(&e.to_string()).into_response(),
        }
    }
    for (candidate, spans) in candidates.iter().zip(per_project) {
        if spans.is_empty() {
            continue;
        }
        if let Some(version) = spans.iter().find_map(|s| s.attrs.get("service.version")) {
            state.note_version(&candidate.id, version);
        }
        let events = candidate
            .state
            .runtime()
            .ingest(spans, hoshizora_runtime::now_unix_nanos());
        for event in events {
            let _ = candidate.state.events.send(event);
        }
    }
    if unplaced > 0 {
        tracing::debug!(spans = unplaced, "dropped spans no project claims");
    }
    tracing::debug!(spans = count, "ingested OTLP export");
    if content_type
        .to_ascii_lowercase()
        .starts_with(otlp::CONTENT_TYPE_JSON)
    {
        ([(header::CONTENT_TYPE, otlp::CONTENT_TYPE_JSON)], "{}").into_response()
    } else {
        (
            [(header::CONTENT_TYPE, otlp::CONTENT_TYPE_PROTOBUF)],
            Bytes::new(),
        )
            .into_response()
    }
}

async fn runtime_stats<Q: GraphQueries>(Proj(state): Proj<Q>) -> Json<Vec<NodeStats>> {
    Json(state.runtime().stats(hoshizora_runtime::now_unix_nanos()))
}

#[derive(Deserialize)]
struct TracesParams {
    limit: Option<usize>,
}

async fn runtime_traces<Q: GraphQueries>(
    Proj(state): Proj<Q>,
    Query(p): Query<TracesParams>,
) -> Json<Vec<Trace>> {
    Json(state.runtime().traces(p.limit.unwrap_or(50)))
}

async fn runtime_trace<Q: GraphQueries>(
    Proj(state): Proj<Q>,
    params: RawPathParams,
) -> Result<Json<Trace>, AppError> {
    let trace_id = path_param(&params, "trace_id")?;
    state
        .runtime()
        .trace(&trace_id)
        .map(Json)
        .ok_or_else(|| not_found("trace"))
}

async fn stream<Q: GraphQueries>(
    Proj(state): Proj<Q>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = state.events.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(|item| match item {
        Ok(event) => match serde_json::to_string(&event) {
            Ok(json) => Some(Ok(Event::default().data(json))),
            Err(_) => None,
        },
        Err(_lagged) => None,
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

async fn static_assets(uri: Uri) -> Response {
    assets::serve(uri.path())
}

/// `effects by kind` summary used by `hoshizora index`.
pub fn effects_by_kind(nodes: &[Node]) -> BTreeMap<String, usize> {
    let mut out = BTreeMap::new();
    for e in nodes.iter().flat_map(|n| &n.effects.direct) {
        let key = serde_json::to_value(e.kind)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_else(|| format!("{:?}", e.kind));
        *out.entry(key).or_insert(0) += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use hoshizora_model::api::{DomainReport, Impact, SearchHit, Story, Subgraph};
    use hoshizora_model::{
        Confidence, Edge, EffectKind, Effects, Language, NodeKind, Span, StateDomain,
    };
    use http_body_util::BodyExt;
    use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
    use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, any_value};
    use opentelemetry_proto::tonic::resource::v1::Resource;
    use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, span};
    use prost::Message;
    use tower::ServiceExt;

    use super::*;

    const FN_ID: &str = "fn:src/lib.rs#Store::create";

    fn node(id: &str, kind: NodeKind, file: Option<&str>, span: Option<(u32, u32)>) -> Node {
        let rest = id.split_once(':').map_or(id, |(_, r)| r);
        let qualified = rest.split_once('#').map_or(rest, |(_, q)| q);
        Node {
            id: id.into(),
            kind,
            name: qualified.rsplit("::").next().unwrap_or(qualified).into(),
            qualified_name: qualified.into(),
            language: None,
            file: file.map(str::to_string),
            span: span.map(|(s, e)| Span {
                start_line: s,
                end_line: e,
            }),
            parent: None,
            function: None,
            effects: Effects::default(),
            entrypoint: None,
            dead: false,
            lines: 0,
        }
    }

    struct Fake {
        graph: Graph,
    }

    impl Fake {
        fn new() -> Self {
            let mut graph = Graph::default();
            graph.meta.root = "/tmp/fake".into();
            graph.meta.indexer = hoshizora_core::INDEXER_VERSION.into();
            let mut function = node(FN_ID, NodeKind::Function, Some("src/lib.rs"), Some((2, 3)));
            function.function = Some(hoshizora_model::FunctionInfo {
                signature: "fn create(&self) -> Session".into(),
                params: vec![hoshizora_model::Param {
                    name: "self".into(),
                    ty: None,
                    default: None,
                }],
                returns: Some("Session".into()),
                doc: Some("Creates a session.".into()),
                is_async: true,
                is_method: true,
                visibility: hoshizora_model::Visibility::Public,
                attributes: vec!["instrument".into()],
                cyclomatic: 2,
                branches: 1,
                loops: 1,
                max_loop_depth: 1,
                recursive: false,
                complexity: hoshizora_model::Complexity {
                    time: "O(n)".into(),
                    space: "O(1)".into(),
                    basis: vec!["one loop".into()],
                },
                fan_in: 1,
                fan_out: 0,
                flows: vec![],
            });
            function.effects = Effects {
                direct: vec![hoshizora_model::Effect {
                    kind: EffectKind::DbRead,
                    target: Some("sessions".into()),
                    via: "sqlx::query".into(),
                    line: 3,
                    loop_depth: 1,
                    awaited: true,
                    rule: "rust.sqlx.query".into(),
                }],
                transitive: vec![EffectKind::DbRead],
            };
            graph.nodes = vec![
                function,
                node("route:POST /api/sessions", NodeKind::Route, None, None),
            ];
            let mut handles = Edge::new(
                "route:POST /api/sessions",
                FN_ID,
                EdgeKind::Handles,
                Confidence::Exact,
            );
            handles.attrs.insert("candidates".into(), "2".into());
            handles
                .attrs
                .insert("matched".into(), "/api/sessions".into());
            graph.edges = vec![
                handles,
                Edge::new(
                    "file:src/lib.rs",
                    FN_ID,
                    EdgeKind::Contains,
                    Confidence::Exact,
                ),
                Edge::new(
                    "file:src/lib.rs",
                    "file:src/other.rs",
                    EdgeKind::Imports,
                    Confidence::Exact,
                ),
            ];
            Self { graph }
        }
    }

    impl NodeLookup for Fake {
        fn node(&self, id: &str) -> Option<&Node> {
            self.graph.nodes.iter().find(|n| n.id == id)
        }
        fn function_at(&self, file: &str, line: u32) -> Option<&Node> {
            self.graph.nodes.iter().find(|n| {
                n.file.as_deref() == Some(file)
                    && n.span
                        .is_some_and(|s| s.start_line <= line && line <= s.end_line)
            })
        }
        fn functions_named(&self, name: &str) -> Vec<&Node> {
            self.graph
                .nodes
                .iter()
                .filter(|n| n.name == name || n.qualified_name == name)
                .collect()
        }
        fn nodes(&self) -> &[Node] {
            &self.graph.nodes
        }
    }

    impl GraphQueries for Fake {
        fn from_graph(graph: Graph) -> Self {
            Self { graph }
        }
        fn graph(&self) -> &Graph {
            &self.graph
        }
        fn incoming(&self, id: &str) -> Vec<Edge> {
            self.graph
                .edges
                .iter()
                .filter(|e| e.to == id)
                .cloned()
                .collect()
        }
        fn outgoing(&self, id: &str) -> Vec<Edge> {
            self.graph
                .edges
                .iter()
                .filter(|e| e.from == id)
                .cloned()
                .collect()
        }
        fn flow(&self, from: &str, _depth: u32, _kinds: &[EdgeKind]) -> Option<Subgraph> {
            self.node(from).map(|_| Subgraph {
                root: from.into(),
                ..Default::default()
            })
        }
        fn story(&self, from: &str, _depth: u32) -> Option<Story> {
            self.node(from).map(|_| Story {
                root: from.into(),
                ..Story::default()
            })
        }
        fn impact(&self, of: &str, _depth: u32) -> Option<Impact> {
            self.node(of).map(|_| Impact {
                target: of.into(),
                affected: vec![],
                entrypoints: vec![],
                edges: vec![],
            })
        }
        fn dead(&self) -> Vec<Node> {
            vec![]
        }
        fn entrypoints(&self) -> Vec<Node> {
            vec![]
        }
        fn with_effect(&self, _kind: EffectKind, _target: Option<&str>) -> Vec<Node> {
            vec![]
        }
        fn search(&self, _query: &str, _limit: usize) -> Vec<SearchHit> {
            vec![]
        }
        fn static_insights(&self) -> Vec<Insight> {
            vec![Insight::IoInLoop {
                node: FN_ID.into(),
                effect: "db_read".into(),
                line: 3,
                observed_calls_per_parent: None,
            }]
        }
        fn states(&self, domain: Option<&str>) -> StatesReport {
            let d = StateDomain {
                id: "domain:src/lib.rs#Status".into(),
                name: "Status".into(),
                file: "src/lib.rs".into(),
                language: Language::Rust,
                values: vec![],
                span: Span::default(),
            };
            StatesReport {
                domains: domain
                    .is_none_or(|q| q == d.name)
                    .then_some(DomainReport {
                        domain: d,
                        sites: vec![],
                    })
                    .into_iter()
                    .collect(),
                columns: Vec::new(),
            }
        }
    }

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "hoshizora-test-{}-{}",
            std::process::id(),
            rand_suffix()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn temp_root() -> PathBuf {
        let dir = temp_dir();
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(
            dir.join("src/lib.rs"),
            "line one\nline two\nline three\nline four\n",
        )
        .unwrap();
        dir
    }

    fn rand_suffix() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    }

    fn fake_indexer() -> crate::projects::Indexer {
        crate::projects::in_process(|_root| Ok(Fake::new().graph))
    }

    fn app_state() -> Arc<AppState<Fake>> {
        AppState::new(
            temp_dir(),
            fake_indexer(),
            RuntimeConfig::default(),
            Default::default(),
        )
    }

    fn path_project(id: &str, root: &Path) -> Project {
        Project {
            id: id.into(),
            name: id.into(),
            source: hoshizora_model::api::ProjectSource::Path {
                path: root.display().to_string(),
            },
            status: ProjectStatus::Indexing,
            error: None,
            commit: None,
            indexed_at: None,
            nodes: 0,
            edges: 0,
            services: vec![],
            tracked_ref: None,
            indexed_ref: None,
            seen_version: None,
            auto_refresh: false,
        }
    }

    fn app() -> (Router, Arc<ProjectState<Fake>>) {
        let app_state = app_state();
        let root = temp_root();
        app_state.insert_ready(path_project("fake", &root), Fake::new(), root, None);
        let state = app_state.project_state(None).unwrap();
        (router(app_state), state)
    }

    async fn get_json(app: &Router, uri: &str) -> (StatusCode, serde_json::Value) {
        let res = app
            .clone()
            .oneshot(Request::get(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, json)
    }

    #[tokio::test]
    async fn graph_and_node_detail_with_encoded_id() {
        let (app, _) = app();
        let (status, graph) = get_json(&app, "/api/graph").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(graph["nodes"].as_array().unwrap().len(), 2);

        let encoded = "fn%3Asrc%2Flib.rs%23Store%3A%3Acreate";
        let (status, detail) = get_json(&app, &format!("/api/nodes/{encoded}")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(detail["node"]["id"], FN_ID);
        assert_eq!(detail["incoming"].as_array().unwrap().len(), 2);
        assert_eq!(detail["source"]["text"], "line two\nline three");
        assert!(detail["stats"].is_null());

        let (status, detail) = get_json(&app, "/api/nodes/route:POST%20/api/sessions").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(detail["node"]["kind"], "route");

        let (status, _) = get_json(&app, "/api/nodes/fn:nope").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn slim_graph_strips_heavy_fields_and_compresses() {
        let (app, state) = app();
        let (status, slim) = get_json(&app, "/api/graph?detail=slim").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(slim["nodes"].as_array().unwrap().len(), 2);
        let edges = slim["edges"].as_array().unwrap();
        assert_eq!(edges.len(), 1, "handles kept, contains/imports dropped");
        assert_eq!(edges[0]["kind"], "handles");
        assert_eq!(edges[0]["attrs"], serde_json::json!({ "candidates": "2" }));
        let f = &slim["nodes"][0]["function"];
        assert_eq!(f["doc"], serde_json::Value::Null);
        assert_eq!(f["signature"], "");
        assert_eq!(f["params"], serde_json::json!([]));
        assert_eq!(f["complexity"]["basis"], serde_json::json!([]));
        assert_eq!(f["complexity"]["time"], "O(n)");
        assert_eq!(
            slim["nodes"][0]["effects"]["direct"],
            serde_json::json!([{
                "kind": "db_read", "target": "sessions", "via": "sqlx::query",
                "line": 3, "loop_depth": 1, "awaited": true, "rule": "rust.sqlx.query"
            }])
        );
        assert_eq!(
            slim["nodes"][0]["effects"]["transitive"],
            serde_json::json!(["db_read"])
        );
        assert!(state.slim_graph_json.len() < state.graph_json.len());

        let res = app
            .oneshot(
                Request::get("/api/graph")
                    .header(header::ACCEPT_ENCODING, "gzip, br")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let enc = res.headers()[header::CONTENT_ENCODING].to_str().unwrap();
        assert!(enc == "gzip" || enc == "br", "{enc}");
    }

    #[tokio::test]
    async fn source_rejects_traversal_and_absolute_paths() {
        let (app, state) = app();
        let (status, src) = get_json(&app, "/api/source?file=src/lib.rs&start=1&end=2").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(src["text"], "line one\nline two");
        for bad in ["../../etc/passwd", "src/../../x", "/etc/passwd"] {
            let (status, _) = get_json(
                &app,
                &format!("/api/source?file={}", bad.replace('/', "%2F")),
            )
            .await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
        }
        let outside = state.root.parent().unwrap().join("outside.txt");
        std::fs::write(&outside, "secret").unwrap();
        std::os::unix::fs::symlink(&outside, state.root.join("link.txt")).unwrap();
        let (status, _) = get_json(&app, "/api/source?file=link.txt").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (status, _) = get_json(&app, "/api/source?file=missing.rs").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    fn kv(key: &str, value: &str) -> KeyValue {
        KeyValue {
            key: key.into(),
            value: Some(AnyValue {
                value: Some(any_value::Value::StringValue(value.into())),
            }),
            ..Default::default()
        }
    }

    fn export(
        trace_id: u8,
        span_id: u8,
        parent: Option<u8>,
        kind: span::SpanKind,
        start: u64,
        end: u64,
        attrs: Vec<KeyValue>,
    ) -> ExportTraceServiceRequest {
        ExportTraceServiceRequest {
            resource_spans: vec![ResourceSpans {
                resource: Some(Resource {
                    attributes: vec![kv("service.name", "svc")],
                    ..Default::default()
                }),
                scope_spans: vec![ScopeSpans {
                    spans: vec![opentelemetry_proto::tonic::trace::v1::Span {
                        trace_id: vec![trace_id; 16],
                        span_id: vec![span_id; 8],
                        parent_span_id: parent.map(|p| vec![p; 8]).unwrap_or_default(),
                        name: "x".into(),
                        kind: kind as i32,
                        start_time_unix_nano: start,
                        end_time_unix_nano: end,
                        attributes: attrs,
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                schema_url: String::new(),
            }],
        }
    }

    async fn post(app: &Router, content_type: &str, body: Vec<u8>) -> StatusCode {
        app.clone()
            .oneshot(
                Request::post("/v1/traces")
                    .header(header::CONTENT_TYPE, content_type)
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap()
            .status()
    }

    #[test]
    fn stats_persist_and_restore_into_a_new_project_state() {
        let app_state = app_state();
        let root = temp_root();
        app_state.insert_ready(path_project("fake", &root), Fake::new(), root.clone(), None);
        let first = app_state.project_state(None).unwrap();
        let path = stats_path(&app_state.data_dir, "fake");

        first.persist_stats(&path);
        assert!(!path.exists());

        let span = RuntimeSpan {
            trace_id: "t".into(),
            span_id: "s".into(),
            parent_span_id: None,
            service: "svc".into(),
            name: "op".into(),
            kind: hoshizora_model::SpanKind::Internal,
            start_unix_nanos: 0,
            end_unix_nanos: 7,
            error: false,
            attrs: BTreeMap::new(),
            node: Some(FN_ID.into()),
            join: None,
        };
        first.runtime().ingest(vec![span], 7);
        persist_all_stats(&app_state);
        assert!(path.exists());

        let second = ProjectState::new(Fake::new(), root, RuntimeConfig::default());
        second.restore_stats(&path);
        second.restore_stats(&path);
        let stats = second.runtime().node_stats(FN_ID, 7).unwrap();
        assert_eq!(stats.count, 1);
        assert_eq!(stats.total_nanos, 7);
        assert_eq!(second.runtime().traces(10).len(), 0);
        let mut rx = second.events.subscribe();
        second.publish_stats();
        assert!(matches!(rx.try_recv().unwrap(), LiveEvent::Stats { stats } if stats.len() == 1));
    }

    #[tokio::test]
    async fn otlp_ingest_updates_stats_traces_and_stream() {
        let (app, state) = app();
        let mut rx = state.events.subscribe();
        let now = hoshizora_runtime::now_unix_nanos();

        let server = export(
            1,
            1,
            None,
            span::SpanKind::Server,
            now,
            now + 100,
            vec![
                kv("http.route", "/api/sessions"),
                kv("http.request.method", "POST"),
            ],
        );
        assert_eq!(
            post(&app, "application/x-protobuf", server.encode_to_vec()).await,
            StatusCode::OK
        );

        let child = export(
            1,
            2,
            Some(1),
            span::SpanKind::Internal,
            now + 10,
            now + 60,
            vec![
                kv("code.file.path", "/abs/src/lib.rs"),
                kv("code.line.number", "2"),
            ],
        );
        assert_eq!(
            post(
                &app,
                "application/json",
                serde_json::to_vec(&child).unwrap()
            )
            .await,
            StatusCode::OK
        );

        assert_eq!(
            post(&app, "application/json", b"not json".to_vec()).await,
            StatusCode::BAD_REQUEST
        );

        let (status, stats) = get_json(&app, "/api/runtime/stats").await;
        assert_eq!(status, StatusCode::OK);
        let by_node: BTreeMap<String, &serde_json::Value> = stats
            .as_array()
            .unwrap()
            .iter()
            .map(|s| (s["node"].as_str().unwrap().to_string(), s))
            .collect();
        assert_eq!(by_node["route:POST /api/sessions"]["count"], 1);
        assert_eq!(by_node["route:POST /api/sessions"]["self_nanos"], 50);
        assert_eq!(by_node[FN_ID]["total_nanos"], 50);

        let (_, traces) = get_json(&app, "/api/runtime/traces?limit=10").await;
        assert_eq!(traces.as_array().unwrap().len(), 1);
        assert_eq!(traces[0]["root_node"], "route:POST /api/sessions");
        let trace_id = traces[0]["trace_id"].as_str().unwrap().to_string();
        let (status, trace) = get_json(&app, &format!("/api/runtime/traces/{trace_id}")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(trace["spans"].as_array().unwrap().len(), 2);
        assert_eq!(trace["spans"][1]["join"], "code_location");

        let (_, detail) = get_json(&app, "/api/nodes/fn%3Asrc%2Flib.rs%23Store%3A%3Acreate").await;
        assert_eq!(detail["stats"]["count"], 1);

        match rx.try_recv().unwrap() {
            LiveEvent::Span { span } => {
                assert_eq!(span.node.as_deref(), Some("route:POST /api/sessions"))
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(rx.try_recv().unwrap(), LiveEvent::Span { .. }));
        state.publish_stats();
        match rx.try_recv().unwrap() {
            LiveEvent::Stats { stats } => assert_eq!(stats.len(), 2),
            other => panic!("{other:?}"),
        }
        state.publish_stats();
        assert!(rx.try_recv().is_err());

        let (_, insights) = get_json(&app, "/api/insights").await;
        assert_eq!(insights["insights"][0]["kind"], "io_in_loop");
    }

    #[tokio::test]
    async fn cors_preflight_from_browser_origin() {
        let (app, _) = app();
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::OPTIONS)
                    .uri("/v1/traces")
                    .header(header::ORIGIN, "http://localhost:5173")
                    .header(header::ACCESS_CONTROL_REQUEST_METHOD, "POST")
                    .header(
                        header::ACCESS_CONTROL_REQUEST_HEADERS,
                        "content-type, x-custom",
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(
            res.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN],
            "http://localhost:5173"
        );
        assert_eq!(
            res.headers()[header::ACCESS_CONTROL_ALLOW_CREDENTIALS],
            "true"
        );
        let headers = res.headers()[header::ACCESS_CONTROL_ALLOW_HEADERS]
            .to_str()
            .unwrap()
            .to_ascii_lowercase();
        assert!(headers.contains("content-type"), "{headers}");
        assert!(headers.contains("x-custom"), "{headers}");
        let methods = res.headers()[header::ACCESS_CONTROL_ALLOW_METHODS]
            .to_str()
            .unwrap();
        for m in ["GET", "POST", "OPTIONS"] {
            assert!(methods.contains(m), "{methods}");
        }

        let res = app
            .oneshot(
                Request::get("/api/graph")
                    .header(header::ORIGIN, "http://localhost:5173")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            res.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN],
            "http://localhost:5173"
        );
        assert_eq!(
            res.headers()[header::ACCESS_CONTROL_ALLOW_CREDENTIALS],
            "true"
        );
    }

    #[tokio::test]
    async fn queries_and_viewer_placeholder() {
        let (app, _) = app();
        let (status, flow) = get_json(
            &app,
            &format!(
                "/api/flow?from={}&kinds=calls,spawns",
                FN_ID.replace('#', "%23")
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(flow["root"], FN_ID);
        let (status, _) = get_json(&app, "/api/flow?from=nope").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = get_json(&app, "/api/flow?from=nope&kinds=bogus").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (status, _) = get_json(&app, "/api/effects?kind=db_write").await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = get_json(&app, "/api/effects?kind=nope").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (status, states) = get_json(&app, "/api/states").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(states["domains"][0]["name"], "Status");
        let (_, states) = get_json(&app, "/api/states?domain=Nope").await;
        assert_eq!(states["domains"].as_array().unwrap().len(), 0);

        for uri in ["/", "/some/spa/route"] {
            let res = app
                .clone()
                .oneshot(Request::get(uri).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(res.status(), StatusCode::OK, "{uri}");
            assert!(
                res.headers()[header::CONTENT_TYPE]
                    .to_str()
                    .unwrap()
                    .starts_with("text/html")
            );
        }
        let res = app
            .oneshot(
                Request::get("/assets/missing.js")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    async fn wait_ready(state: &AppState<Fake>, id: &str) -> Project {
        for _ in 0..200 {
            let p = state.get(id).unwrap();
            if matches!(p.status, ProjectStatus::Ready | ProjectStatus::Failed) {
                return p;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("{id} never finished");
    }

    #[tokio::test]
    async fn prefixed_routes_target_their_project_and_unprefixed_the_first() {
        let app_state = app_state();
        let root_a = temp_root();
        let root_b = temp_root();
        std::fs::write(root_b.join("src/lib.rs"), "b one\nb two\n").unwrap();
        let mut b = Fake::new();
        b.graph.nodes.push(node(
            "fn:src/b.rs#only_in_b",
            NodeKind::Function,
            None,
            None,
        ));
        app_state.insert_ready(path_project("a", &root_a), Fake::new(), root_a, None);
        app_state.insert_ready(path_project("b", &root_b), b, root_b, None);
        let app = router(Arc::clone(&app_state));

        let (status, list) = get_json(&app, "/api/projects").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(list[0]["id"], "a");
        assert_eq!(list[1]["id"], "b");
        assert_eq!(list[1]["nodes"], 3);
        assert_eq!(list[1]["status"], "ready");

        let (_, g) = get_json(&app, "/api/graph").await;
        assert_eq!(g["nodes"].as_array().unwrap().len(), 2);
        let (_, g) = get_json(&app, "/api/projects/a/graph").await;
        assert_eq!(g["nodes"].as_array().unwrap().len(), 2);
        let (_, g) = get_json(&app, "/api/projects/b/graph?detail=slim").await;
        assert_eq!(g["nodes"].as_array().unwrap().len(), 3);

        let (status, d) = get_json(&app, "/api/projects/b/nodes/fn:src/b.rs%23only_in_b").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(d["node"]["id"], "fn:src/b.rs#only_in_b");
        let (status, _) = get_json(&app, "/api/projects/a/nodes/fn:src/b.rs%23only_in_b").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = get_json(&app, "/api/projects/zz/graph").await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        let (_, s) = get_json(&app, "/api/projects/b/source?file=src/lib.rs").await;
        assert_eq!(s["text"], "b one\nb two");
        let (_, s) = get_json(&app, "/api/source?file=src/lib.rs&end=1").await;
        assert_eq!(s["text"], "line one");
        let (status, _) = get_json(&app, "/api/projects/b/source?file=..%2Fx").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        for uri in [
            "/api/projects/b/search?q=x",
            "/api/projects/b/entrypoints",
            "/api/projects/b/dead",
            "/api/projects/b/effects?kind=db_write",
            "/api/projects/b/insights",
            "/api/projects/b/runtime/stats",
            "/api/projects/b/runtime/traces",
            "/api/projects/b/story?from=fn:src/b.rs%23only_in_b",
            "/api/projects/b/flow?from=fn:src/b.rs%23only_in_b",
            "/api/projects/b/impact?of=fn:src/b.rs%23only_in_b",
        ] {
            let (status, _) = get_json(&app, uri).await;
            assert_eq!(status, StatusCode::OK, "{uri}");
        }
        let (status, _) = get_json(&app, "/api/projects/a/flow?from=fn:src/b.rs%23only_in_b").await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        let res = app
            .clone()
            .oneshot(
                Request::delete("/api/projects/a")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NO_CONTENT);
        let (_, g) = get_json(&app, "/api/graph").await;
        assert_eq!(g["nodes"].as_array().unwrap().len(), 3, "b is now first");
        let (status, _) = get_json(&app, "/api/projects/a").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn no_project_is_404_and_pending_project_is_409() {
        let app_state = app_state();
        let app = router(Arc::clone(&app_state));
        let (status, body) = get_json(&app, "/api/graph").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["error"], "project not found");

        let slow = crate::projects::in_process(|_root| {
            std::thread::sleep(Duration::from_millis(300));
            Ok(Fake::new().graph)
        });
        let app_state = AppState::new(
            temp_dir(),
            slow,
            RuntimeConfig::default(),
            Default::default(),
        );
        let app = router(Arc::clone(&app_state));
        let p = app_state.add_path(&temp_root(), None).unwrap();
        assert_eq!(p.status, ProjectStatus::Indexing);
        let (status, body) = get_json(&app, &format!("/api/projects/{}/graph", p.id)).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["status"], "indexing");
        let (status, body) = get_json(&app, "/api/search?q=x").await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["status"], "indexing");
        let ready = wait_ready(&app_state, &p.id).await;
        assert_eq!(ready.status, ProjectStatus::Ready);
        assert_eq!(ready.nodes, 2);
        assert!(ready.indexed_at.unwrap().ends_with('Z'));
        assert!(app_state.graph_path(&p.id).exists());
        let (status, _) = get_json(&app, "/api/search?q=x").await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn registry_round_trips_through_projects_json() {
        let data_dir = temp_dir();
        let root = temp_root();
        let first = AppState::new(
            data_dir.clone(),
            fake_indexer(),
            RuntimeConfig::default(),
            Default::default(),
        );
        let mut p = path_project("fake", &root);
        p.services = vec!["api".into()];
        first.insert_ready(p, Fake::new(), root.clone(), None);
        let mut git = AddProject {
            url: "https://127.0.0.1:9/DorskFR/hoshizora".into(),
            ..Default::default()
        };
        git.services = vec!["web".into()];
        let added = first.add_git(git.clone()).unwrap();
        assert_eq!(added.id, "dorskfr-hoshizora");
        assert_eq!(added.status, ProjectStatus::Cloning);
        assert_eq!(
            first.add_git(git).unwrap().id,
            added.id,
            "same url → same project"
        );
        let other = first
            .add_git(AddProject {
                url: "https://127.0.0.1:9/dorskfr/hoshizora.git".into(),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(other.id, "dorskfr-hoshizora-2");
        wait_ready(&first, &added.id).await;
        wait_ready(&first, &other.id).await;
        assert_eq!(first.get(&added.id).unwrap().status, ProjectStatus::Failed);

        let saved: Vec<Project> =
            serde_json::from_str(&std::fs::read_to_string(data_dir.join("projects.json")).unwrap())
                .unwrap();
        assert_eq!(saved.len(), 3);
        assert_eq!(saved[0].id, "fake");
        assert_eq!(saved[0].services, vec!["api".to_string()]);
        assert_eq!(saved[1].services, vec!["web".to_string()]);
        assert_eq!(saved[1].status, ProjectStatus::Failed);
        assert!(
            saved[1]
                .error
                .as_deref()
                .unwrap()
                .contains("git clone failed")
        );

        let second: Arc<AppState<Fake>> = AppState::new(
            data_dir,
            fake_indexer(),
            RuntimeConfig::default(),
            Default::default(),
        );
        assert_eq!(second.restore().unwrap(), 3);
        let restored = second.list();
        assert_eq!(restored[0].id, "fake");
        assert_eq!(
            restored[0].status,
            ProjectStatus::Ready,
            "graph file was loaded"
        );
        assert_eq!(
            restored[1].status,
            ProjectStatus::Cloning,
            "clone missing → re-clone"
        );
        assert!(second.project_state(Some("fake")).is_ok());
        let err = second
            .project_state(Some("dorskfr-hoshizora"))
            .err()
            .unwrap();
        assert_eq!(err.status, StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn add_project_validates_url_and_returns_202() {
        let app_state = app_state();
        let app = router(Arc::clone(&app_state));
        for bad in [
            "file:///etc",
            "/tmp/repo",
            "../x",
            "http://github.com/a/b",
            "-c x",
        ] {
            let res = app
                .clone()
                .oneshot(
                    Request::post("/api/projects")
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(serde_json::json!({ "url": bad }).to_string()))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(res.status(), StatusCode::BAD_REQUEST, "{bad}");
        }
        let res = app
            .clone()
            .oneshot(
                Request::post("/api/projects")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({ "url": "https://github.com/Tokio-RS/mini-redis", "name": "redis", "ref": "--bad" })
                            .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
        let res = app
            .clone()
            .oneshot(
                Request::post("/api/projects")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({ "url": "https://github.com/Tokio-RS/mini-redis", "name": "redis" })
                            .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::ACCEPTED);
        let body: serde_json::Value =
            serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
        assert_eq!(body["id"], "tokio-rs-mini-redis");
        assert_eq!(body["name"], "redis");
        assert_eq!(body["source"]["kind"], "git");
        assert_eq!(body["status"], "cloning");
        let (status, p) = get_json(&app, "/api/projects/tokio-rs-mini-redis").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(p["id"], "tokio-rs-mini-redis");
        wait_ready(&app_state, "tokio-rs-mini-redis").await;
    }

    fn export_for(service: &str, trace: u8, attrs: Vec<KeyValue>) -> Vec<u8> {
        let mut req = export(trace, 1, None, span::SpanKind::Internal, 10, 20, attrs);
        req.resource_spans[0].resource.as_mut().unwrap().attributes =
            vec![kv("service.name", service)];
        req.encode_to_vec()
    }

    async fn post_to(
        app: &Router,
        uri: &str,
        headers: &[(&str, &str)],
        body: Vec<u8>,
    ) -> StatusCode {
        let mut req = Request::post(uri).header(header::CONTENT_TYPE, "application/x-protobuf");
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        app.clone()
            .oneshot(req.body(Body::from(body)).unwrap())
            .await
            .unwrap()
            .status()
    }

    #[tokio::test]
    async fn otlp_routes_by_service_name_header_query_and_code_location() {
        let app_state = app_state();
        let root_a = temp_root();
        let root_b = temp_root();
        let mut pa = path_project("a", &root_a);
        pa.services = vec!["svc-a".into()];
        app_state.insert_ready(pa, Fake::new(), root_a, None);
        let mut b = Fake::new();
        b.graph.nodes[0].file = Some("src/b.rs".into());
        b.graph.nodes[0].id = "fn:src/b.rs#B::go".into();
        app_state.insert_ready(path_project("b", &root_b), b, root_b, None);
        let app = router(Arc::clone(&app_state));
        let a = app_state.project_state(Some("a")).unwrap();
        let b = app_state.project_state(Some("b")).unwrap();
        let mut rx_a = a.events.subscribe();
        let mut rx_b = b.events.subscribe();

        let loc_b = vec![
            kv("code.file.path", "/x/src/b.rs"),
            kv("code.line.number", "2"),
        ];
        assert_eq!(
            post_to(
                &app,
                "/v1/traces",
                &[],
                export_for("svc-a", 1, loc_b.clone())
            )
            .await,
            StatusCode::OK
        );
        assert_eq!(
            post_to(
                &app,
                "/v1/traces",
                &[("x-hoshizora-project", "b")],
                export_for("other", 2, vec![])
            )
            .await,
            StatusCode::OK
        );
        assert_eq!(
            post_to(
                &app,
                "/v1/traces?project=b",
                &[],
                export_for("other", 3, vec![])
            )
            .await,
            StatusCode::OK
        );
        assert_eq!(
            post_to(&app, "/v1/traces", &[], export_for("other", 4, loc_b)).await,
            StatusCode::OK
        );
        assert_eq!(
            post_to(&app, "/v1/traces", &[], export_for("other", 5, vec![])).await,
            StatusCode::OK
        );
        assert_eq!(
            post_to(
                &app,
                "/v1/traces",
                &[("x-hoshizora-project", "nope")],
                export_for("other", 6, vec![])
            )
            .await,
            StatusCode::NOT_FOUND
        );

        let (_, ta) = get_json(&app, "/api/projects/a/runtime/traces").await;
        let (_, tb) = get_json(&app, "/api/projects/b/runtime/traces").await;
        let ids = |t: &serde_json::Value| -> Vec<String> {
            let mut v: Vec<String> = t
                .as_array()
                .unwrap()
                .iter()
                .map(|t| t["trace_id"].as_str().unwrap().to_string())
                .collect();
            v.sort();
            v
        };
        assert_eq!(ids(&ta), vec!["01".repeat(16)]);
        assert_eq!(
            ids(&tb),
            vec!["02".repeat(16), "03".repeat(16), "04".repeat(16)]
        );
        let (_, unprefixed) = get_json(&app, "/api/runtime/traces").await;
        assert_eq!(ids(&unprefixed), ids(&ta));

        let mut seen_a = 0;
        while rx_a.try_recv().is_ok() {
            seen_a += 1;
        }
        let mut seen_b = 0;
        while rx_b.try_recv().is_ok() {
            seen_b += 1;
        }
        assert_eq!((seen_a, seen_b), (1, 3));
    }

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .current_dir(dir)
            .args(args)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    #[tokio::test]
    async fn clones_indexes_reindexes_and_deletes_a_local_bare_repo() {
        let dir = temp_dir();
        let work = dir.join("work");
        std::fs::create_dir_all(work.join("src")).unwrap();
        std::fs::write(work.join("src/lib.rs"), "pub fn alpha() {}\n").unwrap();
        git(&work, &["init", "-q", "-b", "main"]);
        git(&work, &["add", "."]);
        git(&work, &["commit", "-q", "-m", "one"]);
        let first_commit = git(&work, &["rev-parse", "HEAD"]);
        let bare = dir.join("origin.git");
        git(
            &dir,
            &[
                "clone",
                "-q",
                "--bare",
                work.to_str().unwrap(),
                bare.to_str().unwrap(),
            ],
        );

        let indexer = crate::projects::in_process(|root| crate::build_graph(root, None));
        let app_state: Arc<AppState<crate::queries::GraphIndex>> = AppState::new(
            dir.join("data"),
            indexer,
            RuntimeConfig::default(),
            crate::git::GitOptions {
                allow_file_urls: true,
                timeout: Some(Duration::from_secs(60)),
                ..Default::default()
            },
        );
        let app = router(Arc::clone(&app_state));
        let url = format!("file://{}", bare.display());
        let p = app_state
            .add_git(AddProject {
                url: url.clone(),
                r#ref: Some("main".into()),
                ..Default::default()
            })
            .unwrap();
        assert!(p.id.ends_with("-origin"), "{}", p.id);
        let id = p.id.clone();
        let wait = |id: String| {
            let s = Arc::clone(&app_state);
            async move {
                for _ in 0..400 {
                    let p = s.get(&id).unwrap();
                    if matches!(p.status, ProjectStatus::Ready | ProjectStatus::Failed) {
                        return p;
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                panic!("never finished")
            }
        };
        let ready = wait(p.id.clone()).await;
        assert_eq!(ready.status, ProjectStatus::Ready, "{:?}", ready.error);
        assert_eq!(ready.commit.as_deref(), Some(first_commit.as_str()));
        assert!(ready.nodes > 0);
        assert!(app_state.repo_dir(&p.id).join("src/lib.rs").exists());
        let (status, hits) = get_json(&app, &format!("/api/projects/{id}/search?q=alpha")).await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            hits.as_array()
                .unwrap()
                .iter()
                .any(|h| h["node"]["name"] == "alpha" || h["name"] == "alpha"),
            "{hits}"
        );
        let (status, src) =
            get_json(&app, &format!("/api/projects/{id}/source?file=src/lib.rs")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(src["text"], "pub fn alpha() {}");

        std::fs::write(
            work.join("src/lib.rs"),
            "pub fn alpha() {}\npub fn beta() {}\n",
        )
        .unwrap();
        git(&work, &["commit", "-q", "-am", "two"]);
        git(&work, &["push", "-q", bare.to_str().unwrap(), "main"]);
        let second_commit = git(&work, &["rev-parse", "HEAD"]);
        let res = app
            .clone()
            .oneshot(
                Request::post(format!("/api/projects/{id}/reindex"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::ACCEPTED);
        let ready2 = wait(p.id.clone()).await;
        assert_eq!(ready2.status, ProjectStatus::Ready, "{:?}", ready2.error);
        assert_eq!(ready2.commit.as_deref(), Some(second_commit.as_str()));
        assert!(ready2.nodes > ready.nodes);
        let (_, g) = get_json(&app, &format!("/api/projects/{id}/graph?detail=slim")).await;
        assert!(
            g["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|n| n["name"] == "beta")
        );

        app_state.remove(&id).unwrap();
        assert!(!app_state.repo_dir(&id).exists());
        assert!(!app_state.graph_path(&id).exists());
        assert!(app_state.list().is_empty());
    }

    async fn post_json(app: &Router, uri: &str) -> (StatusCode, serde_json::Value) {
        let res = app
            .clone()
            .oneshot(Request::post(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, json)
    }

    async fn patch_json(app: &Router, uri: &str, body: &str) -> (StatusCode, serde_json::Value) {
        let res = app
            .clone()
            .oneshot(
                Request::patch(uri)
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, json)
    }

    #[tokio::test]
    async fn auto_refresh_is_opt_in_and_persisted() {
        let app_state = app_state();
        let root = temp_root();
        app_state.insert_ready(
            path_project("local", &root),
            Fake::new(),
            root.clone(),
            None,
        );
        let mut git = path_project("remote", &root);
        git.source = hoshizora_model::api::ProjectSource::Git {
            url: "https://127.0.0.1:9/a/b".into(),
            r#ref: None,
        };
        app_state.insert_ready(git, Fake::new(), root, None);
        let app = router(Arc::clone(&app_state));

        let (_, listed) = get_json(&app, "/api/projects").await;
        assert!(
            listed
                .as_array()
                .unwrap()
                .iter()
                .all(|p| p["auto_refresh"] == false),
            "{listed}"
        );

        let (status, body) =
            patch_json(&app, "/api/projects/local", r#"{"auto_refresh":true}"#).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        let (status, body) =
            patch_json(&app, "/api/projects/nope", r#"{"auto_refresh":true}"#).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

        let (status, body) =
            patch_json(&app, "/api/projects/remote", r#"{"auto_refresh":true}"#).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["auto_refresh"], true);
        let (status, body) = patch_json(&app, "/api/projects/remote", "{}").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(
            body["auto_refresh"], true,
            "absent field leaves it unchanged"
        );

        let saved: Vec<Project> = serde_json::from_str(
            &std::fs::read_to_string(app_state.data_dir.join("projects.json")).unwrap(),
        )
        .unwrap();
        let flags: Vec<(&str, bool)> = saved
            .iter()
            .map(|p| (p.id.as_str(), p.auto_refresh))
            .collect();
        assert_eq!(flags, vec![("local", false), ("remote", true)]);

        let (status, body) =
            patch_json(&app, "/api/projects/remote", r#"{"auto_refresh":false}"#).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(!app_state.get("remote").unwrap().auto_refresh);
    }

    #[tokio::test]
    async fn purges_clones_keeps_graphs_and_reindex_restores_them() {
        let dir = temp_dir();
        let work = dir.join("work");
        std::fs::create_dir_all(work.join("src")).unwrap();
        std::fs::write(work.join("src/lib.rs"), "pub fn alpha() {}\n").unwrap();
        git(&work, &["init", "-q", "-b", "main"]);
        git(&work, &["add", "."]);
        git(&work, &["commit", "-q", "-m", "one"]);
        let bare = dir.join("origin.git");
        git(
            &dir,
            &[
                "clone",
                "-q",
                "--bare",
                work.to_str().unwrap(),
                bare.to_str().unwrap(),
            ],
        );
        let local = dir.join("local");
        std::fs::create_dir_all(local.join("src")).unwrap();
        std::fs::write(local.join("src/lib.rs"), "pub fn gamma() {}\n").unwrap();

        let indexer = crate::projects::in_process(|root| crate::build_graph(root, None));
        let make_state = || {
            AppState::<crate::queries::GraphIndex>::new(
                dir.join("data"),
                Arc::clone(&indexer),
                RuntimeConfig::default(),
                crate::git::GitOptions {
                    allow_file_urls: true,
                    timeout: Some(Duration::from_secs(60)),
                    ..Default::default()
                },
            )
        };
        let app_state = make_state();
        let app = router(Arc::clone(&app_state));
        let git_id = app_state
            .add_git(AddProject {
                url: format!("file://{}", bare.display()),
                r#ref: Some("main".into()),
                ..Default::default()
            })
            .unwrap()
            .id;
        let path_id = app_state.add_path(&local, None).unwrap().id;
        let wait = |s: &Arc<AppState<crate::queries::GraphIndex>>| {
            let s = Arc::clone(s);
            async move {
                for _ in 0..400 {
                    let all = s.list();
                    if all
                        .iter()
                        .all(|p| matches!(p.status, ProjectStatus::Ready | ProjectStatus::Failed))
                    {
                        return all;
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                panic!("never finished")
            }
        };
        for p in wait(&app_state).await {
            assert_eq!(p.status, ProjectStatus::Ready, "{:?}", p.error);
        }
        let repo = app_state.repo_dir(&git_id);
        assert!(repo.join("src/lib.rs").exists());

        let (status, storage) = get_json(&app, "/api/storage").await;
        assert_eq!(status, StatusCode::OK, "{storage}");
        let entry = |s: &serde_json::Value, id: &str| {
            s["projects"]
                .as_array()
                .unwrap()
                .iter()
                .find(|p| p["id"] == id)
                .cloned()
                .unwrap()
        };
        let before = entry(&storage, &git_id);
        assert!(before["clone_bytes"].as_u64().unwrap() > 0, "{storage}");
        assert!(before["graph_bytes"].as_u64().unwrap() > 0);
        assert!(entry(&storage, &path_id)["clone_bytes"].is_null());
        assert_eq!(storage["clone_bytes"], before["clone_bytes"]);
        assert!(storage["disk"]["total_bytes"].as_u64().unwrap() > 0);

        let (status, body) = post_json(&app, &format!("/api/projects/{path_id}/purge")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert!(local.join("src/lib.rs").exists());

        let (status, purged) = post_json(&app, &format!("/api/projects/{git_id}/purge")).await;
        assert_eq!(status, StatusCode::OK, "{purged}");
        assert_eq!(purged["id"], git_id);
        assert_eq!(purged["freed_bytes"], before["clone_bytes"]);
        assert!(!repo.exists());
        let p = app_state.get(&git_id).unwrap();
        assert_eq!(p.status, ProjectStatus::Ready);
        assert!(p.nodes > 0);
        let (status, hits) =
            get_json(&app, &format!("/api/projects/{git_id}/search?q=alpha")).await;
        assert_eq!(status, StatusCode::OK);
        assert!(!hits.as_array().unwrap().is_empty());
        let (status, err) = get_json(
            &app,
            &format!("/api/projects/{git_id}/source?file=src/lib.rs"),
        )
        .await;
        assert_eq!(status, StatusCode::GONE, "{err}");
        assert_eq!(err["error"], SOURCE_UNAVAILABLE);
        let (_, storage) = get_json(&app, "/api/storage").await;
        assert!(entry(&storage, &git_id)["clone_bytes"].is_null());
        assert_eq!(storage["clone_bytes"], 0);
        let (status, again) = post_json(&app, &format!("/api/projects/{git_id}/purge")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(again["freed_bytes"], 0);

        let restarted = make_state();
        assert_eq!(restarted.restore().unwrap(), 2);
        let restored = restarted.get(&git_id).unwrap();
        assert_eq!(
            restored.status,
            ProjectStatus::Ready,
            "{:?}",
            restored.error
        );
        assert!(!repo.exists());

        let (status, _) = post_json(&app, &format!("/api/projects/{git_id}/reindex")).await;
        assert_eq!(status, StatusCode::ACCEPTED);
        for p in wait(&app_state).await {
            assert_eq!(p.status, ProjectStatus::Ready, "{:?}", p.error);
        }
        assert!(repo.join("src/lib.rs").exists());
        let (status, src) = get_json(
            &app,
            &format!("/api/projects/{git_id}/source?file=src/lib.rs"),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{src}");
        assert_eq!(src["text"], "pub fn alpha() {}");

        let (status, report) = post_json(&app, "/api/purge").await;
        assert_eq!(status, StatusCode::OK, "{report}");
        let ids: Vec<&str> = report["projects"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, [git_id.as_str()]);
        assert!(report["freed_bytes"].as_u64().unwrap() > 0);
        assert_eq!(report["freed_bytes"], report["projects"][0]["freed_bytes"]);
        assert!(!repo.exists());
        assert!(local.join("src/lib.rs").exists());
        assert_eq!(
            app_state.get(&path_id).unwrap().status,
            ProjectStatus::Ready
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
