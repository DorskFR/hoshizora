mod assets;
mod git;
mod projects;
mod queries;
mod server;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use anyhow::Context;
use clap::{Parser, Subcommand};
use hoshizora_core::{Index, Packs};
use hoshizora_model::NodeKind;
use hoshizora_runtime::{NodeLookup, RuntimeConfig};
use tracing_subscriber::EnvFilter;

use crate::git::GitOptions;
use crate::projects::{AppState, Indexer};
use crate::queries::{GraphIndex, GraphQueries, parse_edge_kinds, parse_effect_kind};
use crate::server::DEFAULT_DEPTH;

const DEFAULT_GRAPH: &str = ".hoshizora/graph.json";
const DEFAULT_DATA_DIR: &str = ".hoshizora";

#[derive(Parser)]
#[command(
    name = "hoshizora",
    version,
    about = "Map a codebase as a graph of flows and effects, animated by OpenTelemetry"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Parse a source tree and write the graph.
    Index {
        /// Root of the source tree.
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Where to write the graph.
        #[arg(short, long, default_value = DEFAULT_GRAPH)]
        output: PathBuf,
        /// Extra effect/route packs (`*.toml`) overriding the builtin ones.
        #[arg(long)]
        packs: Option<PathBuf>,
    },
    /// Serve the API, the OTLP receiver and the viewer.
    Serve {
        /// Pre-built graph to serve as the first project (default `.hoshizora/graph.json`
        /// when it exists and nothing else is registered).
        #[arg(long)]
        graph: Option<PathBuf>,
        /// Source root of `--graph`; defaults to the root recorded in the graph.
        #[arg(long)]
        root: Option<PathBuf>,
        /// Holds `projects.json`, clones (`repos/`) and graphs (`graphs/`).
        #[arg(long, default_value = DEFAULT_DATA_DIR)]
        data_dir: PathBuf,
        /// Git URL or local path to register at startup; repeatable.
        #[arg(long = "project")]
        projects: Vec<String>,
        /// Bind address; use 0.0.0.0 inside a container.
        #[arg(long, default_value = "127.0.0.1")]
        host: std::net::IpAddr,
        #[arg(long, default_value_t = 7317)]
        port: u16,
        /// Standard OTLP/HTTP port; skipped with a warning when busy.
        #[arg(long, default_value_t = 4318)]
        otlp_port: u16,
        /// Rebuild the graph from `--root` before serving.
        #[arg(long)]
        reindex: bool,
        /// Extra packs used with `--reindex`.
        #[arg(long)]
        packs: Option<PathBuf>,
    },
    /// Compare two revisions (source trees or graph files) and print JSON.
    Diff {
        base: PathBuf,
        head: PathBuf,
        /// Write the report here instead of stdout.
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Extra packs used when indexing a source tree.
        #[arg(long)]
        packs: Option<PathBuf>,
    },
    /// Query a graph and print JSON.
    Query {
        #[arg(long, default_value = DEFAULT_GRAPH)]
        graph: PathBuf,
        #[command(subcommand)]
        query: QueryCommand,
    },
}

#[derive(Subcommand)]
enum QueryCommand {
    /// Everything a node triggers.
    Flow {
        id: String,
        #[arg(long, default_value_t = DEFAULT_DEPTH)]
        depth: u32,
        /// Comma-separated edge kinds (`calls,spawns,requests`); all non-structural kinds when omitted.
        #[arg(long, default_value = "")]
        kinds: String,
    },
    /// Everything that reaches a node.
    Impact {
        id: String,
        #[arg(long, default_value_t = DEFAULT_DEPTH)]
        depth: u32,
    },
    /// Nodes unreachable from any entrypoint.
    Dead,
    /// Nodes with a direct effect of this kind (`db_write`, `network`, ...).
    Effects {
        kind: String,
        #[arg(long)]
        target: Option<String>,
    },
    /// Static findings.
    Insights,
    /// Fuzzy search over names.
    Search {
        q: String,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// One node with its edges.
    Node { id: String },
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_target(false)
        .init();
    let cli = Cli::parse();
    match cli.command {
        Command::Index {
            path,
            output,
            packs,
        } => cmd_index(&path, &output, packs.as_deref()),
        Command::Serve {
            graph,
            root,
            data_dir,
            projects,
            host,
            port,
            otlp_port,
            reindex,
            packs,
        } => tokio::runtime::Runtime::new()?.block_on(cmd_serve(ServeArgs {
            graph,
            root,
            data_dir,
            projects,
            host,
            port,
            otlp_port,
            reindex,
            packs,
        })),
        Command::Diff {
            base,
            head,
            output,
            packs,
        } => cmd_diff(&base, &head, output.as_deref(), packs.as_deref()),
        Command::Query { graph, query } => cmd_query(&graph, query),
    }
}

/// A directory is indexed like `hoshizora index`; anything else is loaded as a graph file.
fn load_or_build(path: &Path, packs: Option<&Path>) -> anyhow::Result<hoshizora_model::Graph> {
    if path.is_dir() {
        build_graph(path, packs)
    } else {
        hoshizora_core::load(path).with_context(|| format!("loading {}", path.display()))
    }
}

fn cmd_diff(
    base: &Path,
    head: &Path,
    output: Option<&Path>,
    packs: Option<&Path>,
) -> anyhow::Result<()> {
    let started = Instant::now();
    let base = Index::new(load_or_build(base, packs)?);
    let head = Index::new(load_or_build(head, packs)?);
    let report = hoshizora_core::diff(&base, &head);
    let json = serde_json::to_string_pretty(&report)?;
    match output {
        Some(path) => {
            std::fs::write(path, json)?;
            tracing::info!(
                "diff written to {} in {:.2?}",
                path.display(),
                started.elapsed()
            );
        }
        None => println!("{json}"),
    }
    Ok(())
}

fn load_packs(extra: Option<&Path>) -> anyhow::Result<Packs> {
    let mut packs = Packs::builtin();
    if let Some(dir) = extra {
        packs
            .load_dir(dir)
            .with_context(|| format!("loading packs from {}", dir.display()))?;
    }
    Ok(packs)
}

fn build_graph(root: &Path, extra_packs: Option<&Path>) -> anyhow::Result<hoshizora_model::Graph> {
    let root = root
        .canonicalize()
        .with_context(|| format!("root {}", root.display()))?;
    let packs = load_packs(extra_packs)?;
    let files = hoshizora_adapters::index_tree(&root)?;
    Ok(hoshizora_core::build_graph(&root, files, &packs))
}

fn cmd_index(path: &Path, output: &Path, packs: Option<&Path>) -> anyhow::Result<()> {
    let started = Instant::now();
    let graph = build_graph(path, packs)?;
    if let Some(parent) = output.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    hoshizora_core::save(&graph, output)?;
    let functions = graph
        .nodes
        .iter()
        .filter(|n| n.kind == NodeKind::Function)
        .count();
    let dead = graph.nodes.iter().filter(|n| n.dead).count();
    println!("indexed {} in {:.2?}", graph.meta.root, started.elapsed());
    println!("  files      {}", graph.meta.files);
    println!("  nodes      {} ({functions} functions)", graph.nodes.len());
    println!("  edges      {}", graph.edges.len());
    println!("  dead       {dead}");
    let effects = server::effects_by_kind(&graph.nodes);
    if effects.is_empty() {
        println!("  effects    none");
    } else {
        println!("  effects");
        for (kind, count) in effects {
            println!("    {kind:<12} {count}");
        }
    }
    for w in &graph.meta.warnings {
        tracing::warn!("{w}");
    }
    println!("  written to {}", output.display());
    Ok(())
}

struct ServeArgs {
    graph: Option<PathBuf>,
    root: Option<PathBuf>,
    data_dir: PathBuf,
    projects: Vec<String>,
    host: std::net::IpAddr,
    port: u16,
    otlp_port: u16,
    reindex: bool,
    packs: Option<PathBuf>,
}

async fn cmd_serve(args: ServeArgs) -> anyhow::Result<()> {
    let packs = args.packs.clone();
    let indexer: Indexer = Arc::new(move |root: &Path| build_graph(root, packs.as_deref()));
    let state: Arc<AppState<GraphIndex>> = AppState::new(
        args.data_dir.clone(),
        indexer,
        RuntimeConfig::default(),
        GitOptions::from_env(),
    );
    let restored = state.restore()?;
    if restored > 0 {
        tracing::info!(projects = restored, "restored projects.json");
    }
    let legacy = args.graph.clone().or_else(|| {
        let default = PathBuf::from(DEFAULT_GRAPH);
        (restored == 0 && args.projects.is_empty() && (default.exists() || args.reindex))
            .then_some(default)
    });
    if let Some(graph_path) = legacy {
        serve_legacy_graph(
            &state,
            &graph_path,
            args.root.clone(),
            args.reindex,
            args.packs.as_deref(),
        )?;
    }
    for spec in &args.projects {
        let project = if projects::is_url(spec) {
            state
                .add_git(hoshizora_model::api::AddProject {
                    url: spec.clone(),
                    ..Default::default()
                })
                .map_err(|e| anyhow::anyhow!("--project {spec}: {}", e.message))?
        } else {
            state.add_path(Path::new(spec), None)?
        };
        tracing::info!(project = project.id, status = ?project.status, "registered");
    }
    server::spawn_stats_ticker(Arc::clone(&state));
    let app = server::router(Arc::clone(&state));

    let main_addr = SocketAddr::from((args.host, args.port));
    let main = tokio::net::TcpListener::bind(main_addr)
        .await
        .with_context(|| format!("binding {main_addr}"))?;
    tracing::info!("hoshizora listening on http://{main_addr}");

    let otlp_addr = SocketAddr::from((args.host, args.otlp_port));
    let otlp = if args.otlp_port == args.port {
        None
    } else {
        match tokio::net::TcpListener::bind(otlp_addr).await {
            Ok(l) => {
                tracing::info!("OTLP/HTTP receiver also on http://{otlp_addr}/v1/traces");
                Some(l)
            }
            Err(e) => {
                tracing::warn!(
                    "port {} busy ({e}); export to http://{main_addr} instead (OTEL_EXPORTER_OTLP_ENDPOINT)",
                    args.otlp_port
                );
                None
            }
        }
    };

    let main_srv = axum::serve(main, app.clone());
    match otlp {
        Some(l) => {
            let otlp_srv = axum::serve(l, app);
            tokio::try_join!(main_srv.into_future(), otlp_srv.into_future())?;
        }
        None => main_srv.await?,
    }
    Ok(())
}

/// `serve --graph X [--root Y]`: loads (or rebuilds) one graph and registers it as the first
/// project, so unprefixed `/api/...` routes keep working.
fn serve_legacy_graph(
    state: &Arc<AppState<GraphIndex>>,
    graph_path: &Path,
    root: Option<PathBuf>,
    reindex: bool,
    packs: Option<&Path>,
) -> anyhow::Result<()> {
    let graph = if reindex {
        let root = root.clone().unwrap_or_else(|| PathBuf::from("."));
        let graph = build_graph(&root, packs)?;
        if let Some(parent) = graph_path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        hoshizora_core::save(&graph, graph_path)?;
        graph
    } else {
        hoshizora_core::load(graph_path).with_context(|| {
            format!(
                "loading {} (run `hoshizora index` first)",
                graph_path.display()
            )
        })?
    };
    let root = root.unwrap_or_else(|| PathBuf::from(&graph.meta.root));
    tracing::info!(
        nodes = graph.nodes.len(),
        edges = graph.edges.len(),
        root = %root.display(),
        "graph loaded"
    );
    let id = state
        .list()
        .into_iter()
        .find(|p| state.root_of(p) == root)
        .map(|p| p.id)
        .unwrap_or_else(|| projects::path_slug(&root));
    let stored = state.graph_path(&id);
    if graph_path.canonicalize().ok() != stored.canonicalize().ok() {
        if let Some(parent) = stored.parent() {
            std::fs::create_dir_all(parent)?;
        }
        hoshizora_core::save(&graph, &stored)?;
    }
    let project = hoshizora_model::api::Project {
        name: id.clone(),
        id,
        source: hoshizora_model::api::ProjectSource::Path {
            path: root.display().to_string(),
        },
        status: hoshizora_model::api::ProjectStatus::Ready,
        error: None,
        commit: None,
        indexed_at: None,
        nodes: 0,
        edges: 0,
        services: Vec::new(),
    };
    state.insert_ready(project, GraphIndex::new(graph), root, Some(0));
    Ok(())
}

fn cmd_query(graph_path: &Path, query: QueryCommand) -> anyhow::Result<()> {
    let graph = hoshizora_core::load(graph_path).with_context(|| {
        format!(
            "loading {} (run `hoshizora index` first)",
            graph_path.display()
        )
    })?;
    let index = GraphIndex(Index::new(graph));
    let value = match query {
        QueryCommand::Flow { id, depth, kinds } => {
            let kinds = parse_edge_kinds(&kinds)?;
            serde_json::to_value(
                index
                    .flow(&id, depth, &kinds)
                    .with_context(|| format!("unknown node {id}"))?,
            )?
        }
        QueryCommand::Impact { id, depth } => serde_json::to_value(
            index
                .impact(&id, depth)
                .with_context(|| format!("unknown node {id}"))?,
        )?,
        QueryCommand::Dead => serde_json::to_value(index.dead())?,
        QueryCommand::Effects { kind, target } => {
            serde_json::to_value(index.with_effect(parse_effect_kind(&kind)?, target.as_deref()))?
        }
        QueryCommand::Insights => serde_json::to_value(hoshizora_model::api::Insights {
            insights: index.static_insights(),
        })?,
        QueryCommand::Search { q, limit } => serde_json::to_value(index.search(&q, limit))?,
        QueryCommand::Node { id } => {
            let node = index
                .node(&id)
                .with_context(|| format!("unknown node {id}"))?
                .clone();
            serde_json::to_value(hoshizora_model::api::NodeDetail {
                incoming: index.incoming(&id),
                outgoing: index.outgoing(&id),
                node,
                source: None,
                stats: None,
            })?
        }
    };
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}
