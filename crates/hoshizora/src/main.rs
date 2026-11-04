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
        /// Project to register at startup, `[name=]<url>[@ref]` or a local path; repeatable,
        /// or comma-separated in `HOSHIZORA_PROJECTS`. An already registered URL or path keeps
        /// its clone and graph and takes the declared name, services and ref.
        #[arg(long = "project", env = "HOSHIZORA_PROJECTS", value_delimiter = ',')]
        projects: Vec<String>,
        /// JSON array of `{url, ref?, name?, services?}` or `{path, name?}` entries to register
        /// at startup, same rules as `--project`.
        #[arg(long, env = "HOSHIZORA_PROJECTS_FILE")]
        projects_file: Option<PathBuf>,
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
        /// Minutes between remote checks of git projects with auto-refresh enabled
        /// (`git ls-remote`); a moved ref triggers a rebuild. 0 disables the check.
        #[arg(long, env = "HOSHIZORA_REFRESH_INTERVAL", default_value_t = 15)]
        refresh_interval: u64,
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
    /// Score diff reports against known review corrections (see docs/coverage.md).
    Coverage {
        /// JSON file of cases: repo + base/head revs or prebuilt graphs, plus locations.
        #[arg(long)]
        cases: PathBuf,
        /// Write the JSON report here; the table always goes to stdout.
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Where revisions are extracted and indexed (default: a `hoshizora-coverage` dir in
        /// the system temp dir); reused across runs.
        #[arg(long)]
        scratch: Option<PathBuf>,
        /// Extra packs used when indexing.
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
    /// Value domains (enums, literal unions, const lists) and the branches on them, with the
    /// values each branch misses.
    States {
        #[arg(long)]
        domain: Option<String>,
    },
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
            projects_file,
            host,
            port,
            otlp_port,
            reindex,
            packs,
            refresh_interval,
        } => tokio::runtime::Runtime::new()?.block_on(cmd_serve(ServeArgs {
            graph,
            root,
            data_dir,
            projects,
            projects_file,
            host,
            port,
            otlp_port,
            reindex,
            packs,
            refresh_interval,
        })),
        Command::Diff {
            base,
            head,
            output,
            packs,
        } => cmd_diff(&base, &head, output.as_deref(), packs.as_deref()),
        Command::Coverage {
            cases,
            output,
            scratch,
            packs,
        } => cmd_coverage(
            &cases,
            output.as_deref(),
            scratch.as_deref(),
            packs.as_deref(),
        ),
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

/// Indexes in a `hoshizora index` child process so the indexing peak is returned to the OS
/// when it exits instead of staying in the server's heap.
fn child_indexer(exe: PathBuf, packs: Option<PathBuf>) -> Indexer {
    Arc::new(move |root: &Path, output: &Path| {
        let mut partial = output.as_os_str().to_owned();
        partial.push(".partial");
        let partial = PathBuf::from(partial);
        let mut cmd = std::process::Command::new(&exe);
        cmd.arg("index").arg(root).arg("--output").arg(&partial);
        if let Some(packs) = &packs {
            cmd.arg("--packs").arg(packs);
        }
        let out = cmd
            .stdin(std::process::Stdio::null())
            .output()
            .with_context(|| format!("running {}", exe.display()))?;
        if !out.status.success() {
            let _ = std::fs::remove_file(&partial);
            let stderr = String::from_utf8_lossy(&out.stderr);
            let stderr = stderr.trim();
            let tail = &stderr[stderr.floor_char_boundary(stderr.len().saturating_sub(2000))..];
            anyhow::bail!("indexer {}: {tail}", out.status);
        }
        std::fs::rename(&partial, output)
            .with_context(|| format!("moving the graph to {}", output.display()))?;
        hoshizora_core::load(output)
    })
}

struct ServeArgs {
    graph: Option<PathBuf>,
    root: Option<PathBuf>,
    data_dir: PathBuf,
    projects: Vec<String>,
    projects_file: Option<PathBuf>,
    host: std::net::IpAddr,
    port: u16,
    otlp_port: u16,
    reindex: bool,
    packs: Option<PathBuf>,
    refresh_interval: u64,
}

async fn cmd_serve(args: ServeArgs) -> anyhow::Result<()> {
    let exe = std::env::current_exe().context("locating the hoshizora binary")?;
    let indexer = child_indexer(exe, args.packs.clone());
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
    let mut specs = args
        .projects
        .iter()
        .filter(|s| !s.trim().is_empty())
        .map(|s| projects::parse_spec(s).with_context(|| format!("--project {s}")))
        .collect::<anyhow::Result<Vec<_>>>()?;
    if let Some(file) = &args.projects_file {
        specs.extend(projects::load_specs_file(file)?);
    }
    let legacy = args.graph.clone().or_else(|| {
        let default = PathBuf::from(DEFAULT_GRAPH);
        (restored == 0 && specs.is_empty() && (default.exists() || args.reindex)).then_some(default)
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
    for spec in specs {
        match state.register(spec.clone()) {
            Ok(project) => {
                tracing::info!(project = project.id, status = ?project.status, "registered")
            }
            Err(e) => tracing::error!(error = %e, "registering {spec:?}"),
        }
    }
    server::spawn_stats_ticker(Arc::clone(&state));
    if args.refresh_interval > 0 {
        state.spawn_refresh(std::time::Duration::from_secs(args.refresh_interval * 60));
    }
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

    let main_srv = axum::serve(main, app.clone()).with_graceful_shutdown(shutdown_signal());
    match otlp {
        Some(l) => {
            let otlp_srv = axum::serve(l, app).with_graceful_shutdown(shutdown_signal());
            tokio::try_join!(main_srv.into_future(), otlp_srv.into_future())?;
        }
        None => main_srv.await?,
    }
    server::persist_all_stats(&state);
    tracing::info!("runtime stats saved");
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(e) => {
                tracing::warn!(error = %e, "SIGTERM handler unavailable");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
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
        tracked_ref: None,
        indexed_ref: None,
        seen_version: None,
        auto_refresh: false,
    };
    state.insert_ready(project, GraphIndex::new(graph), root, Some(0));
    Ok(())
}

#[derive(serde::Deserialize)]
#[serde(untagged)]
enum CasesFile {
    List(Vec<CaseSpec>),
    Wrapped { cases: Vec<CaseSpec> },
}

/// Either `repo` + `base` + `head` revisions, or `base_graph` + `head_graph` files (`repo` is
/// then optional and only used to derive `in_diff` from `git diff`).
#[derive(serde::Deserialize)]
struct CaseSpec {
    name: Option<String>,
    repo: Option<PathBuf>,
    base: Option<String>,
    head: Option<String>,
    base_graph: Option<PathBuf>,
    head_graph: Option<PathBuf>,
    locations: Vec<CaseLocation>,
}

#[derive(serde::Deserialize)]
struct CaseLocation {
    file: String,
    line: u32,
    in_diff: Option<bool>,
}

fn cmd_coverage(
    cases_path: &Path,
    output: Option<&Path>,
    scratch: Option<&Path>,
    packs: Option<&Path>,
) -> anyhow::Result<()> {
    let text = std::fs::read_to_string(cases_path)
        .with_context(|| format!("reading {}", cases_path.display()))?;
    let specs = match serde_json::from_str::<CasesFile>(&text)
        .with_context(|| format!("parsing {}", cases_path.display()))?
    {
        CasesFile::List(c) | CasesFile::Wrapped { cases: c } => c,
    };
    let scratch = scratch.map_or_else(
        || std::env::temp_dir().join("hoshizora-coverage"),
        Path::to_path_buf,
    );
    let mut results = Vec::with_capacity(specs.len());
    for (i, spec) in specs.into_iter().enumerate() {
        let name = spec
            .name
            .clone()
            .unwrap_or_else(|| format!("case-{}", i + 1));
        let started = Instant::now();
        let result =
            coverage_case(&name, &spec, &scratch, packs).with_context(|| format!("case {name}"))?;
        tracing::info!(case = name, elapsed = ?started.elapsed(), "scored");
        results.push(result);
    }
    let report = hoshizora_core::coverage::Report {
        summary: hoshizora_core::coverage::summarize(&results),
        cases: results,
    };
    if let Some(path) = output {
        std::fs::write(path, serde_json::to_string_pretty(&report)?)?;
    }
    print!("{}", hoshizora_core::coverage::table(&report));
    if output.is_none() {
        println!("{}", serde_json::to_string_pretty(&report)?);
    }
    Ok(())
}

fn coverage_case(
    name: &str,
    spec: &CaseSpec,
    scratch: &Path,
    packs: Option<&Path>,
) -> anyhow::Result<hoshizora_core::coverage::CaseResult> {
    let graph_for = |rev: Option<&str>, graph: Option<&Path>, side: &str| match (graph, rev) {
        (Some(path), _) => hoshizora_core::load(path).with_context(|| path.display().to_string()),
        (None, Some(rev)) => {
            let repo = spec
                .repo
                .as_deref()
                .context("`repo` is required with revisions")?;
            let dir = scratch.join(projects::path_slug(repo)).join(rev);
            if !dir.join(".hoshizora-archived").exists() {
                let _ = std::fs::remove_dir_all(&dir);
                git::archive(repo, rev, &dir)?;
                std::fs::write(dir.join(".hoshizora-archived"), "")?;
            }
            build_graph(&dir, packs)
        }
        (None, None) => anyhow::bail!("{side} needs a revision or a graph file"),
    };
    let base = Index::new(graph_for(
        spec.base.as_deref(),
        spec.base_graph.as_deref(),
        "base",
    )?);
    let head = Index::new(graph_for(
        spec.head.as_deref(),
        spec.head_graph.as_deref(),
        "head",
    )?);
    let report = hoshizora_core::diff(&base, &head);
    let needs_hunks = spec.locations.iter().any(|l| l.in_diff.is_none());
    let hunks = match (&spec.repo, &spec.base, &spec.head) {
        (Some(repo), Some(base), Some(head)) if needs_hunks => {
            git::changed_lines(repo, base, head)?
        }
        _ => Default::default(),
    };
    let locations: Vec<_> = spec
        .locations
        .iter()
        .map(|l| hoshizora_core::coverage::Location {
            in_diff: l.in_diff.unwrap_or_else(|| {
                hunks
                    .get(&l.file)
                    .is_some_and(|ranges| ranges.iter().any(|&(a, b)| a <= l.line && l.line <= b))
            }),
            file: l.file.clone(),
            line: l.line,
        })
        .collect();
    Ok(hoshizora_core::coverage::evaluate(
        name, &head, &report, &locations,
    ))
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
        QueryCommand::States { domain } => serde_json::to_value(index.states(domain.as_deref()))?,
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
