//! The project registry: `projects.json`, clones under `repos/`, graphs under `graphs/`, and the
//! background clone + index jobs that move a project to `ready`.

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};

use anyhow::Context;
use hoshizora_model::Graph;
use hoshizora_model::api::{
    AddProject, DiskSpace, Project, ProjectSource, ProjectStatus, ProjectStorage, PurgeReport,
    Purged, Storage,
};
use hoshizora_runtime::RuntimeConfig;

use crate::git::{self, GitOptions};
use crate::queries::GraphQueries;
use crate::server::{AppError, ProjectState};

pub type Indexer = Arc<dyn Fn(&Path) -> anyhow::Result<Graph> + Send + Sync>;

pub struct ProjectEntry<Q: GraphQueries> {
    pub project: Project,
    /// Present once the graph is loaded; cleared while reindexing replaces it.
    pub state: Option<Arc<ProjectState<Q>>>,
}

pub struct AppState<Q: GraphQueries> {
    pub data_dir: PathBuf,
    pub git: GitOptions,
    config: RuntimeConfig,
    indexer: Indexer,
    projects: RwLock<Vec<ProjectEntry<Q>>>,
}

impl<Q: GraphQueries> AppState<Q> {
    pub fn new(
        data_dir: PathBuf,
        indexer: Indexer,
        config: RuntimeConfig,
        git: GitOptions,
    ) -> Arc<Self> {
        Arc::new(Self {
            data_dir,
            git,
            config,
            indexer,
            projects: RwLock::new(Vec::new()),
        })
    }

    fn registry_path(&self) -> PathBuf {
        self.data_dir.join("projects.json")
    }

    pub fn repo_dir(&self, id: &str) -> PathBuf {
        self.data_dir.join("repos").join(id)
    }

    pub fn graph_path(&self, id: &str) -> PathBuf {
        self.data_dir.join("graphs").join(format!("{id}.json"))
    }

    fn read(&self) -> RwLockReadGuard<'_, Vec<ProjectEntry<Q>>> {
        self.projects.read().unwrap_or_else(|e| e.into_inner())
    }

    fn write(&self) -> RwLockWriteGuard<'_, Vec<ProjectEntry<Q>>> {
        self.projects.write().unwrap_or_else(|e| e.into_inner())
    }

    pub fn list(&self) -> Vec<Project> {
        self.read().iter().map(|e| e.project.clone()).collect()
    }

    pub fn get(&self, id: &str) -> Option<Project> {
        self.read()
            .iter()
            .find(|e| e.project.id == id)
            .map(|e| e.project.clone())
    }

    pub fn ready_states(&self) -> Vec<Arc<ProjectState<Q>>> {
        self.read().iter().filter_map(|e| e.state.clone()).collect()
    }

    /// `(id, services, state)` of every project that can receive spans.
    pub fn candidates(&self) -> Vec<(String, Vec<String>, Arc<ProjectState<Q>>)> {
        self.read()
            .iter()
            .filter_map(|e| {
                e.state
                    .clone()
                    .map(|s| (e.project.id.clone(), e.project.services.clone(), s))
            })
            .collect()
    }

    /// The project a request targets: `id`, or the first registered one.
    pub fn project_state(&self, id: Option<&str>) -> Result<Arc<ProjectState<Q>>, AppError> {
        let projects = self.read();
        let entry = match id {
            Some(id) => projects
                .iter()
                .find(|e| e.project.id == id)
                .ok_or_else(|| AppError::not_found("project"))?,
            None => projects
                .first()
                .ok_or_else(|| AppError::not_found("project"))?,
        };
        entry.state.clone().ok_or_else(|| {
            AppError::not_ready(
                entry.project.status,
                entry
                    .project
                    .error
                    .as_deref()
                    .unwrap_or("project is not ready"),
            )
        })
    }

    fn save(&self) -> anyhow::Result<()> {
        let projects = self.list();
        std::fs::create_dir_all(&self.data_dir)?;
        let path = self.registry_path();
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(&projects)?)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    fn persist(&self) {
        if let Err(e) = self.save() {
            tracing::error!(error = %e, "saving projects.json");
        }
    }

    /// Reads `projects.json`; projects with a graph on disk are loaded, the rest are rebuilt in
    /// the background. Returns the number of restored projects.
    pub fn restore(self: &Arc<Self>) -> anyhow::Result<usize> {
        let path = self.registry_path();
        if !path.exists() {
            return Ok(0);
        }
        let text = std::fs::read_to_string(&path).with_context(|| path.display().to_string())?;
        let projects: Vec<Project> =
            serde_json::from_str(&text).with_context(|| path.display().to_string())?;
        let count = projects.len();
        for mut project in projects {
            let graph_path = self.graph_path(&project.id);
            let root = self.root_of(&project);
            let root_required = matches!(project.source, ProjectSource::Path { .. });
            let loaded = (graph_path.exists() && (!root_required || root.exists()))
                .then(|| hoshizora_core::load(&graph_path).ok())
                .flatten();
            match loaded {
                Some(graph) => {
                    project.status = ProjectStatus::Ready;
                    project.error = None;
                    let state = ProjectState::new(Q::from_graph(graph), root, self.config.clone());
                    fill_counts(&mut project, &state);
                    self.write().push(ProjectEntry {
                        project,
                        state: Some(state),
                    });
                }
                None => {
                    let id = project.id.clone();
                    project.status = initial_status(&project.source);
                    project.error = None;
                    self.write().push(ProjectEntry {
                        project,
                        state: None,
                    });
                    self.spawn_job(id);
                }
            }
        }
        Ok(count)
    }

    pub fn root_of(&self, project: &Project) -> PathBuf {
        match &project.source {
            ProjectSource::Git { .. } => self.repo_dir(&project.id),
            ProjectSource::Path { path } => PathBuf::from(path),
        }
    }

    fn unique_id(&self, base: &str) -> String {
        let projects = self.read();
        let taken = |id: &str| projects.iter().any(|e| e.project.id == id);
        if !taken(base) {
            return base.to_string();
        }
        (2..)
            .map(|n| format!("{base}-{n}"))
            .find(|id| !taken(id))
            .unwrap_or_default()
    }

    /// Registers a loaded graph as a ready project at `position` (or at the end).
    pub fn insert_ready(
        &self,
        mut project: Project,
        queries: Q,
        root: PathBuf,
        position: Option<usize>,
    ) -> Project {
        let state = ProjectState::new(queries, root, self.config.clone());
        project.status = ProjectStatus::Ready;
        project.error = None;
        fill_counts(&mut project, &state);
        let stored = self.graph_path(&project.id);
        if !stored.exists()
            && let Err(e) = stored
                .parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .map_err(anyhow::Error::from)
                .and_then(|()| hoshizora_core::save(state.queries.graph(), &stored))
        {
            tracing::warn!(error = %e, "saving {}", stored.display());
        }
        {
            let mut projects = self.write();
            projects.retain(|e| e.project.id != project.id);
            let entry = ProjectEntry {
                project: project.clone(),
                state: Some(state),
            };
            match position {
                Some(i) if i <= projects.len() => projects.insert(i, entry),
                _ => projects.push(entry),
            }
        }
        self.persist();
        project
    }

    /// Registers a git project and starts cloning + indexing it. The same URL twice returns the
    /// existing project.
    pub fn add_git(self: &Arc<Self>, req: AddProject) -> Result<Project, AppError> {
        let url = req.url.trim().to_string();
        git::validate_url(&url, self.git.allow_file_urls).map_err(AppError::bad_request)?;
        if let Some(r) = req.r#ref.as_deref() {
            git::validate_ref(r).map_err(AppError::bad_request)?;
        }
        let existing = self.read().iter().find_map(|e| match &e.project.source {
            ProjectSource::Git { url: u, .. }
                if u.trim_end_matches('/') == url.trim_end_matches('/') =>
            {
                Some(e.project.clone())
            }
            _ => None,
        });
        if let Some(existing) = existing {
            return Ok(existing);
        }
        let id = self.unique_id(&git::slug_for_url(&url));
        let project = Project {
            name: req
                .name
                .filter(|n| !n.trim().is_empty())
                .unwrap_or_else(|| id.clone()),
            id: id.clone(),
            source: ProjectSource::Git {
                url,
                r#ref: req.r#ref.filter(|r| !r.is_empty()),
            },
            status: ProjectStatus::Cloning,
            error: None,
            commit: None,
            indexed_at: None,
            nodes: 0,
            edges: 0,
            services: req.services,
        };
        self.write().push(ProjectEntry {
            project: project.clone(),
            state: None,
        });
        self.persist();
        self.spawn_job(id);
        Ok(project)
    }

    /// Registers a local source tree and indexes it in the background.
    pub fn add_path(
        self: &Arc<Self>,
        path: &Path,
        name: Option<String>,
    ) -> anyhow::Result<Project> {
        let path = path
            .canonicalize()
            .with_context(|| format!("project path {}", path.display()))?;
        let existing = self.read().iter().find_map(|e| match &e.project.source {
            ProjectSource::Path { path: p } if Path::new(p) == path => Some(e.project.clone()),
            _ => None,
        });
        if let Some(existing) = existing {
            return Ok(existing);
        }
        let id = self.unique_id(&path_slug(&path));
        let project = Project {
            name: name.unwrap_or_else(|| id.clone()),
            id: id.clone(),
            source: ProjectSource::Path {
                path: path.display().to_string(),
            },
            status: ProjectStatus::Indexing,
            error: None,
            commit: None,
            indexed_at: None,
            nodes: 0,
            edges: 0,
            services: Vec::new(),
        };
        self.write().push(ProjectEntry {
            project: project.clone(),
            state: None,
        });
        self.persist();
        self.spawn_job(id);
        Ok(project)
    }

    pub fn reindex(self: &Arc<Self>, id: &str) -> Result<Project, AppError> {
        let project = {
            let mut projects = self.write();
            let entry = projects
                .iter_mut()
                .find(|e| e.project.id == id)
                .ok_or_else(|| AppError::not_found("project"))?;
            if busy(entry.project.status) {
                return Err(AppError::not_ready(
                    entry.project.status,
                    "project is already being rebuilt",
                ));
            }
            entry.project.status = initial_status(&entry.project.source);
            entry.project.error = None;
            entry.project.clone()
        };
        self.persist();
        self.spawn_job(id.to_string());
        Ok(project)
    }

    /// Unregisters the project and deletes its clone and graph.
    pub fn remove(&self, id: &str) -> Result<(), AppError> {
        let removed = {
            let mut projects = self.write();
            let i = projects
                .iter()
                .position(|e| e.project.id == id)
                .ok_or_else(|| AppError::not_found("project"))?;
            projects.remove(i).project
        };
        self.persist();
        let _ = std::fs::remove_file(self.graph_path(id));
        if matches!(removed.source, ProjectSource::Git { .. }) {
            let _ = std::fs::remove_dir_all(self.repo_dir(id));
        }
        Ok(())
    }

    /// Deletes the clone of an idle git project; the registration and graph stay, and the next
    /// reindex clones again. The registry lock is held while deleting so no job can start on
    /// the half-removed tree.
    pub fn purge(&self, id: &str) -> Result<Purged, AppError> {
        let projects = self.write();
        let entry = projects
            .iter()
            .find(|e| e.project.id == id)
            .ok_or_else(|| AppError::not_found("project"))?;
        self.purge_entry(entry)
    }

    fn purge_entry(&self, entry: &ProjectEntry<Q>) -> Result<Purged, AppError> {
        let project = &entry.project;
        if matches!(project.source, ProjectSource::Path { .. }) {
            return Err(AppError::bad_request(
                "a path project has no clone to purge",
            ));
        }
        if busy(project.status) {
            return Err(AppError::not_ready(
                project.status,
                "project is being cloned or indexed",
            ));
        }
        let dir = self.repo_dir(&project.id);
        let freed_bytes = dir_size(&dir);
        if dir.exists() {
            std::fs::remove_dir_all(&dir).map_err(|e| AppError::internal(format!("{e}")))?;
        }
        Ok(Purged {
            id: project.id.clone(),
            freed_bytes,
        })
    }

    /// Purges every git project that is not cloning or indexing.
    pub fn purge_all(&self) -> PurgeReport {
        let projects = self.write();
        let purged: Vec<Purged> = projects
            .iter()
            .filter(|e| matches!(e.project.source, ProjectSource::Git { .. }))
            .filter(|e| !busy(e.project.status))
            .filter_map(|e| self.purge_entry(e).ok())
            .collect();
        PurgeReport {
            freed_bytes: purged.iter().map(|p| p.freed_bytes).sum(),
            projects: purged,
        }
    }

    /// Walks `repos/` and `graphs/`, so it costs a directory traversal per clone; the project
    /// list itself never pays for it.
    pub fn storage(&self) -> Storage {
        let projects: Vec<ProjectStorage> = self
            .list()
            .iter()
            .map(|p| {
                let clone_bytes = match p.source {
                    ProjectSource::Git { .. } => {
                        let dir = self.repo_dir(&p.id);
                        dir.exists().then(|| dir_size(&dir))
                    }
                    ProjectSource::Path { .. } => None,
                };
                ProjectStorage {
                    id: p.id.clone(),
                    clone_bytes,
                    graph_bytes: dir_size(&self.graph_path(&p.id)),
                }
            })
            .collect();
        Storage {
            clone_bytes: projects.iter().filter_map(|p| p.clone_bytes).sum(),
            graph_bytes: projects.iter().map(|p| p.graph_bytes).sum(),
            disk: disk_space(&self.data_dir),
            projects,
        }
    }

    fn set_status(&self, id: &str, status: ProjectStatus, error: Option<String>) {
        if let Some(entry) = self.write().iter_mut().find(|e| e.project.id == id) {
            entry.project.status = status;
            entry.project.error = error;
        }
        self.persist();
    }

    fn spawn_job(self: &Arc<Self>, id: String) {
        let state = Arc::clone(self);
        tokio::spawn(async move {
            match state.run_job(&id).await {
                Ok(()) => tracing::info!(project = id, "project ready"),
                Err(e) => {
                    tracing::warn!(project = id, error = %e, "project failed");
                    state.set_status(&id, ProjectStatus::Failed, Some(e.to_string()));
                }
            }
        });
    }

    async fn run_job(self: &Arc<Self>, id: &str) -> anyhow::Result<()> {
        let project = self.get(id).context("project was removed")?;
        let root = self.root_of(&project);
        let mut commit = None;
        if let ProjectSource::Git { url, r#ref } = &project.source {
            self.set_status(id, ProjectStatus::Cloning, None);
            if root.join(".git").exists() {
                git::update(&self.git, url, r#ref.as_deref(), &root).await?;
            } else {
                let _ = std::fs::remove_dir_all(&root);
                git::clone(&self.git, url, r#ref.as_deref(), &root).await?;
            }
            commit = Some(git::head(&self.git, &root).await?);
        }
        if self.get(id).is_none() {
            anyhow::bail!("project was removed");
        }
        self.set_status(id, ProjectStatus::Indexing, None);
        let indexer = Arc::clone(&self.indexer);
        let graph_path = self.graph_path(id);
        let index_root = root.clone();
        let config = self.config.clone();
        let state = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
            let graph = indexer(&index_root)?;
            if let Some(parent) = graph_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            hoshizora_core::save(&graph, &graph_path)?;
            Ok(ProjectState::new(Q::from_graph(graph), index_root, config))
        })
        .await
        .context("index task panicked")??;
        let mut projects = self.write();
        let entry = projects
            .iter_mut()
            .find(|e| e.project.id == id)
            .context("project was removed")?;
        entry.project.status = ProjectStatus::Ready;
        entry.project.error = None;
        entry.project.commit = commit;
        entry.project.indexed_at = Some(rfc3339_now());
        fill_counts(&mut entry.project, &state);
        entry.state = Some(state);
        drop(projects);
        self.persist();
        Ok(())
    }
}

/// Bytes under `path` (a file or a tree), symlinks counted as themselves and never followed.
pub fn dir_size(path: &Path) -> u64 {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return 0;
    };
    if !meta.is_dir() {
        return meta.len();
    }
    let mut total = 0;
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.is_dir() {
                stack.push(entry.path());
            } else {
                total += meta.len();
            }
        }
    }
    total
}

#[cfg(unix)]
fn disk_space(path: &Path) -> Option<DiskSpace> {
    let probe = if path.exists() {
        path
    } else {
        path.parent().filter(|p| p.exists())?
    };
    let st = rustix::fs::statvfs(probe).ok()?;
    Some(DiskSpace {
        total_bytes: st.f_blocks.saturating_mul(st.f_frsize),
        free_bytes: st.f_bavail.saturating_mul(st.f_frsize),
    })
}

#[cfg(not(unix))]
fn disk_space(_path: &Path) -> Option<DiskSpace> {
    None
}

fn busy(status: ProjectStatus) -> bool {
    matches!(status, ProjectStatus::Cloning | ProjectStatus::Indexing)
}

fn initial_status(source: &ProjectSource) -> ProjectStatus {
    match source {
        ProjectSource::Git { .. } => ProjectStatus::Cloning,
        ProjectSource::Path { .. } => ProjectStatus::Indexing,
    }
}

fn fill_counts<Q: GraphQueries>(project: &mut Project, state: &ProjectState<Q>) {
    let graph = state.queries.graph();
    project.nodes = graph.nodes.len();
    project.edges = graph.edges.len();
    if project.indexed_at.is_none() {
        project.indexed_at = Some(rfc3339_now());
    }
}

pub fn path_slug(path: &Path) -> String {
    git::slugify(
        &path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
    )
}

/// Looks like a git URL rather than a local path (`--project` accepts both).
pub fn is_url(spec: &str) -> bool {
    git::validate_url(spec, true).is_ok()
}

/// One project to register at startup: a git URL with the same fields as the add API, or a
/// local source tree.
#[derive(Debug, Clone, PartialEq)]
pub enum ProjectSpec {
    Git(AddProject),
    Path { path: PathBuf, name: Option<String> },
}

/// `[name=]<url>[@ref]` or `[name=]<path>`. The ref is the part after the last `@` when it
/// contains neither `/` nor `:`, so `git@host:owner/repo` and `ssh://git@host/...` stay intact;
/// refs with a slash need the projects file.
pub fn parse_spec(spec: &str) -> anyhow::Result<ProjectSpec> {
    let spec = spec.trim();
    let (name, rest) = match spec.split_once('=') {
        Some((n, rest))
            if !n.is_empty()
                && n.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ' ')) =>
        {
            (Some(n.trim().to_string()), rest)
        }
        _ => (None, spec),
    };
    if rest.is_empty() {
        anyhow::bail!("empty project spec");
    }
    let (url, r#ref) = match rest.rsplit_once('@') {
        Some((url, r)) if !r.is_empty() && !r.contains('/') && !r.contains(':') => {
            (url, Some(r.to_string()))
        }
        _ => (rest, None),
    };
    if is_url(url) {
        if let Some(r) = &r#ref {
            git::validate_ref(r)?;
        }
        return Ok(ProjectSpec::Git(AddProject {
            url: url.to_string(),
            r#ref,
            name,
            services: Vec::new(),
        }));
    }
    if is_url(rest) {
        return Ok(ProjectSpec::Git(AddProject {
            url: rest.to_string(),
            r#ref: None,
            name,
            services: Vec::new(),
        }));
    }
    Ok(ProjectSpec::Path {
        path: PathBuf::from(rest),
        name,
    })
}

#[derive(serde::Deserialize)]
struct SpecEntry {
    url: Option<String>,
    path: Option<PathBuf>,
    r#ref: Option<String>,
    name: Option<String>,
    #[serde(default)]
    services: Vec<String>,
}

/// JSON array of `{url, ref?, name?, services?}` (the add API body) or `{path, name?}`.
pub fn load_specs_file(path: &Path) -> anyhow::Result<Vec<ProjectSpec>> {
    let text = std::fs::read_to_string(path).with_context(|| path.display().to_string())?;
    let entries: Vec<SpecEntry> =
        serde_json::from_str(&text).with_context(|| path.display().to_string())?;
    entries
        .into_iter()
        .enumerate()
        .map(|(i, e)| match (e.url, e.path) {
            (Some(url), None) => Ok(ProjectSpec::Git(AddProject {
                url,
                r#ref: e.r#ref,
                name: e.name,
                services: e.services,
            })),
            (None, Some(path)) => Ok(ProjectSpec::Path { path, name: e.name }),
            _ => anyhow::bail!(
                "{}: entry {i} needs exactly one of `url` or `path`",
                path.display()
            ),
        })
        .collect()
}

impl<Q: GraphQueries> AppState<Q> {
    /// Adds the project unless an equal URL or path is already registered; cloning and
    /// indexing run in the background either way.
    pub fn register(self: &Arc<Self>, spec: ProjectSpec) -> anyhow::Result<Project> {
        match spec {
            ProjectSpec::Git(req) => self
                .add_git(req)
                .map_err(|e| anyhow::anyhow!("{}", e.message)),
            ProjectSpec::Path { path, name } => self.add_path(&path, name),
        }
    }
}

pub fn rfc3339_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    rfc3339(secs)
}

/// UTC timestamp with second precision (Howard Hinnant's civil-from-days).
pub fn rfc3339(unix_secs: u64) -> String {
    let days = i64::try_from(unix_secs / 86_400).unwrap_or(0);
    let rem = unix_secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_known_instants() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(rfc3339(1_761_868_799), "2025-10-30T23:59:59Z");
    }

    fn git_spec(url: &str, r#ref: Option<&str>, name: Option<&str>) -> ProjectSpec {
        ProjectSpec::Git(AddProject {
            url: url.into(),
            r#ref: r#ref.map(Into::into),
            name: name.map(Into::into),
            services: Vec::new(),
        })
    }

    #[test]
    fn specs() {
        assert_eq!(
            parse_spec("https://github.com/a/b").unwrap(),
            git_spec("https://github.com/a/b", None, None)
        );
        assert_eq!(
            parse_spec("https://github.com/a/b@v1.2").unwrap(),
            git_spec("https://github.com/a/b", Some("v1.2"), None)
        );
        assert_eq!(
            parse_spec("mini-redis=https://github.com/tokio-rs/mini-redis@master").unwrap(),
            git_spec(
                "https://github.com/tokio-rs/mini-redis",
                Some("master"),
                Some("mini-redis")
            )
        );
        assert_eq!(
            parse_spec("git@github.com:a/b.git").unwrap(),
            git_spec("git@github.com:a/b.git", None, None)
        );
        assert_eq!(
            parse_spec("ssh://git@host/a/b@main").unwrap(),
            git_spec("ssh://git@host/a/b", Some("main"), None)
        );
        assert_eq!(
            parse_spec("docs=./src").unwrap(),
            ProjectSpec::Path {
                path: "./src".into(),
                name: Some("docs".into())
            }
        );
        assert_eq!(
            parse_spec("/tmp/repo").unwrap(),
            ProjectSpec::Path {
                path: "/tmp/repo".into(),
                name: None
            }
        );
        assert!(parse_spec("").is_err());
        assert!(parse_spec("https://github.com/a/b@--bad").is_err());
    }

    #[test]
    fn specs_file() {
        let dir = std::env::temp_dir().join(format!("hoshizora-specs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("projects.json");
        std::fs::write(
            &file,
            r#"[{"url":"https://github.com/a/b","ref":"main","services":["b"]},{"path":"/tmp/x","name":"x"}]"#,
        )
        .unwrap();
        let specs = load_specs_file(&file).unwrap();
        assert_eq!(specs.len(), 2);
        assert!(
            matches!(&specs[0], ProjectSpec::Git(a) if a.services == ["b"] && a.r#ref.as_deref() == Some("main"))
        );
        assert!(matches!(&specs[1], ProjectSpec::Path { name: Some(n), .. } if n == "x"));
        std::fs::write(&file, r#"[{"name":"nothing"}]"#).unwrap();
        assert!(load_specs_file(&file).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn configured_projects_are_registered_once_and_indexed() {
        let dir = std::env::temp_dir().join(format!(
            "hoshizora-configured-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let work = dir.join("work");
        std::fs::create_dir_all(work.join("src")).unwrap();
        std::fs::write(work.join("src/lib.rs"), "pub fn alpha() {}\n").unwrap();
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .current_dir(&work)
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
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "one"]);

        let indexer: Indexer = Arc::new(|root: &Path| crate::build_graph(root, None));
        let make_state = || {
            AppState::<crate::queries::GraphIndex>::new(
                dir.join("data"),
                Arc::clone(&indexer),
                RuntimeConfig::default(),
                GitOptions {
                    allow_file_urls: true,
                    timeout: Some(std::time::Duration::from_secs(60)),
                    ..Default::default()
                },
            )
        };
        let url = format!("file://{}", work.display());
        let specs = vec![
            parse_spec(&format!("example={url}@main")).unwrap(),
            ProjectSpec::Path {
                path: work.clone(),
                name: None,
            },
        ];

        let state = make_state();
        for spec in specs.clone() {
            state.register(spec).unwrap();
        }
        for spec in specs.clone() {
            state.register(spec).unwrap();
        }
        let listed = state.list();
        assert_eq!(listed.len(), 2, "{listed:?}");
        assert_eq!(listed[0].name, "example");
        assert!(
            matches!(&listed[0].source, ProjectSource::Git { r#ref: Some(r), .. } if r == "main")
        );
        for _ in 0..400 {
            if state
                .list()
                .iter()
                .all(|p| matches!(p.status, ProjectStatus::Ready | ProjectStatus::Failed))
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        for p in state.list() {
            assert_eq!(p.status, ProjectStatus::Ready, "{:?}", p.error);
            assert!(p.nodes > 0);
        }

        let restarted = make_state();
        assert_eq!(restarted.restore().unwrap(), 2);
        for spec in specs {
            restarted.register(spec).unwrap();
        }
        assert_eq!(restarted.list().len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn url_or_path() {
        assert!(is_url("https://github.com/a/b"));
        assert!(is_url("git@github.com:a/b"));
        assert!(!is_url("./src"));
        assert!(!is_url("/tmp/repo"));
        assert_eq!(path_slug(Path::new("/tmp/My Repo")), "my-repo");
    }
}
