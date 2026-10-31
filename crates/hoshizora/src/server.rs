//! HTTP API from `docs/api.md`, the OTLP/HTTP receiver and the embedded viewer.

use std::collections::BTreeMap;
use std::convert::Infallible;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, Method, StatusCode, Uri, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use futures::Stream;
use hoshizora_model::api::{Insights, NodeDetail, Source};
use hoshizora_model::{EdgeKind, Graph, Insight, LiveEvent, Node, NodeStats};
use hoshizora_runtime::{NodeLookup, Runtime, RuntimeConfig, Trace, otlp};
use serde::Deserialize;
use tokio::sync::broadcast;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;
use tower_http::compression::CompressionLayer;
use tower_http::cors::{AllowHeaders, AllowOrigin, CorsLayer};

use crate::assets;
use crate::queries::{GraphQueries, parse_edge_kinds, parse_effect_kind};

pub struct AppState<Q: GraphQueries> {
    pub queries: Arc<Q>,
    pub runtime: Mutex<Runtime<Arc<Q>>>,
    pub events: broadcast::Sender<LiveEvent>,
    pub root: PathBuf,
    /// `/api/graph` bodies, serialised once: the graph is immutable per process.
    graph_json: Bytes,
    slim_graph_json: Bytes,
}

impl<Q: GraphQueries + 'static> AppState<Q> {
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
}

pub fn router<Q: GraphQueries + 'static>(state: Arc<AppState<Q>>) -> Router {
    let api = Router::new()
        .route("/api/graph", get(graph::<Q>))
        .route("/api/nodes/{*id}", get(node_detail::<Q>))
        .route("/api/source", get(source::<Q>))
        .route("/api/search", get(search::<Q>))
        .route("/api/entrypoints", get(entrypoints::<Q>))
        .route("/api/flow", get(flow::<Q>))
        .route("/api/impact", get(impact::<Q>))
        .route("/api/dead", get(dead::<Q>))
        .route("/api/effects", get(effects::<Q>))
        .route("/api/insights", get(insights::<Q>))
        .route("/api/runtime/stats", get(runtime_stats::<Q>))
        .route("/api/runtime/traces", get(runtime_traces::<Q>))
        .route("/api/runtime/traces/{trace_id}", get(runtime_trace::<Q>))
        .layer(CompressionLayer::new());
    Router::new()
        .merge(api)
        .route("/api/runtime/stream", get(stream::<Q>))
        .route("/v1/traces", post(traces_ingest::<Q>))
        .fallback(static_assets)
        .layer(cors())
        .with_state(state)
}

/// Browser OTel exporters send credentials, which a wildcard origin would make the browser reject.
fn cors() -> CorsLayer {
    CorsLayer::new()
        .allow_origin(AllowOrigin::mirror_request())
        .allow_credentials(true)
        .allow_headers(AllowHeaders::mirror_request())
        .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
        .max_age(Duration::from_secs(3600))
}

/// Emits a `Stats` live event every second while the server runs.
pub fn spawn_stats_ticker<Q: GraphQueries + 'static>(state: Arc<AppState<Q>>) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        loop {
            tick.tick().await;
            state.publish_stats();
        }
    });
}

type AppError = (StatusCode, String);

fn not_found(what: &str) -> AppError {
    (StatusCode::NOT_FOUND, format!("{what} not found"))
}

fn bad_request(msg: impl Into<String>) -> AppError {
    (StatusCode::BAD_REQUEST, msg.into())
}

#[derive(Deserialize)]
struct GraphParams {
    detail: Option<String>,
}

async fn graph<Q: GraphQueries>(
    State(state): State<Arc<AppState<Q>>>,
    Query(p): Query<GraphParams>,
) -> Response {
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
    }
}

async fn node_detail<Q: GraphQueries>(
    State(state): State<Arc<AppState<Q>>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<NodeDetail>, AppError> {
    let id = decode_id(&id);
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
    State(state): State<Arc<AppState<Q>>>,
    Query(p): Query<SourceParams>,
) -> Result<Json<Source>, AppError> {
    let start = p.start.unwrap_or(1).max(1);
    let end = p.end.unwrap_or(u32::MAX).max(start);
    read_source(&state.root, &p.file, start, end).map(Json)
}

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

async fn search<Q: GraphQueries>(
    State(state): State<Arc<AppState<Q>>>,
    Query(p): Query<SearchParams>,
) -> Response {
    Json(state.queries.search(&p.q, p.limit.unwrap_or(20))).into_response()
}

async fn entrypoints<Q: GraphQueries>(State(state): State<Arc<AppState<Q>>>) -> Json<Vec<Node>> {
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
    State(state): State<Arc<AppState<Q>>>,
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
    State(state): State<Arc<AppState<Q>>>,
    Query(p): Query<ImpactParams>,
) -> Result<Response, AppError> {
    state
        .queries
        .impact(&p.of, p.depth.unwrap_or(DEFAULT_DEPTH))
        .map(|s| Json(s).into_response())
        .ok_or_else(|| not_found("node"))
}

async fn dead<Q: GraphQueries>(State(state): State<Arc<AppState<Q>>>) -> Json<Vec<Node>> {
    Json(state.queries.dead())
}

#[derive(Deserialize)]
struct EffectsParams {
    kind: String,
    target: Option<String>,
}

async fn effects<Q: GraphQueries>(
    State(state): State<Arc<AppState<Q>>>,
    Query(p): Query<EffectsParams>,
) -> Result<Json<Vec<Node>>, AppError> {
    let kind = parse_effect_kind(&p.kind).map_err(|e| bad_request(e.to_string()))?;
    Ok(Json(state.queries.with_effect(kind, p.target.as_deref())))
}

async fn insights<Q: GraphQueries>(State(state): State<Arc<AppState<Q>>>) -> Json<Insights> {
    Json(Insights {
        insights: all_insights(&state),
    })
}

pub fn all_insights<Q: GraphQueries>(state: &AppState<Q>) -> Vec<Insight> {
    let runtime = state
        .runtime()
        .insights(hoshizora_runtime::now_unix_nanos());
    let mut out = state.queries.static_insights();
    let (never, rest): (Vec<Insight>, Vec<Insight>) = runtime
        .into_iter()
        .partition(|i| matches!(i, Insight::NeverObserved { .. }));
    out.extend(rest);
    out.extend(never);
    out
}

async fn traces_ingest<Q: GraphQueries>(
    State(state): State<Arc<AppState<Q>>>,
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
            return (StatusCode::BAD_REQUEST, e.to_string()).into_response();
        }
    };
    let count = spans.len();
    let events = state
        .runtime()
        .ingest(spans, hoshizora_runtime::now_unix_nanos());
    for event in events {
        let _ = state.events.send(event);
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

async fn runtime_stats<Q: GraphQueries>(
    State(state): State<Arc<AppState<Q>>>,
) -> Json<Vec<NodeStats>> {
    Json(state.runtime().stats(hoshizora_runtime::now_unix_nanos()))
}

#[derive(Deserialize)]
struct TracesParams {
    limit: Option<usize>,
}

async fn runtime_traces<Q: GraphQueries>(
    State(state): State<Arc<AppState<Q>>>,
    Query(p): Query<TracesParams>,
) -> Json<Vec<Trace>> {
    Json(state.runtime().traces(p.limit.unwrap_or(50)))
}

async fn runtime_trace<Q: GraphQueries>(
    State(state): State<Arc<AppState<Q>>>,
    axum::extract::Path(trace_id): axum::extract::Path<String>,
) -> Result<Json<Trace>, AppError> {
    state
        .runtime()
        .trace(&trace_id)
        .map(Json)
        .ok_or_else(|| not_found("trace"))
}

async fn stream<Q: GraphQueries>(
    State(state): State<Arc<AppState<Q>>>,
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
    use hoshizora_model::api::{Impact, SearchHit, Subgraph};
    use hoshizora_model::{Confidence, Edge, EffectKind, Effects, NodeKind, Span};
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
    }

    fn temp_root() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "hoshizora-test-{}-{}",
            std::process::id(),
            rand_suffix()
        ));
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

    fn app() -> (Router, Arc<AppState<Fake>>) {
        let state = AppState::new(Fake::new(), temp_root(), RuntimeConfig::default());
        (router(Arc::clone(&state)), state)
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
}
