//! The project registry: `projects.json`, clones under `repos/`, graphs under `graphs/`, the
//! background clone + index jobs that move a project to `ready`, and the periodic remote check
//! that reindexes an opted-in git project when its tracked ref moves.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::Duration;

use anyhow::Context;
use hoshizora_model::Graph;
use hoshizora_model::api::{
    AddProject, DiskSpace, Project, ProjectSource, ProjectStatus, ProjectStorage, PurgeReport,
    Purged, Storage, UpdateProject,
};
use hoshizora_runtime::RuntimeConfig;

use crate::git::{self, GitOptions};
use crate::queries::GraphQueries;
use crate::server::{AppError, ProjectState};

/// Builds the graph of a source root, writes it to the output path and returns it.
pub type Indexer = Arc<dyn Fn(&Path, &Path) -> anyhow::Result<Graph> + Send + Sync>;

/// An [`Indexer`] that builds the graph in this process.
#[cfg(test)]
pub fn in_process(
    build: impl Fn(&Path) -> anyhow::Result<Graph> + Send + Sync + 'static,
) -> Indexer {
    Arc::new(move |root: &Path, output: &Path| {
        let graph = build(root)?;
        hoshizora_core::save(&graph, output)?;
        Ok(graph)
    })
}

/// Resolves a ref on a remote to a commit; the production implementation shells out to
/// `git ls-remote`, tests answer from a table.
pub trait Remote: Send + Sync {
    fn ls_remote<'a>(
        &'a self,
        git: &'a GitOptions,
        url: &'a str,
        r#ref: &'a str,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<Option<String>>> + Send + 'a>>;
}

struct GitRemote;

impl Remote for GitRemote {
    fn ls_remote<'a>(
        &'a self,
        git: &'a GitOptions,
        url: &'a str,
        r#ref: &'a str,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<Option<String>>> + Send + 'a>> {
        Box::pin(git::ls_remote(git, url, r#ref))
    }
}

/// Outcome of one remote check of a project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refresh {
    /// Not a git project, or already cloning/indexing.
    Skipped,
    /// The tracked ref still points at the indexed commit.
    Unchanged,
    /// The ref moved (or a version tag was adopted); a rebuild is queued.
    Reindexing,
}

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
    remote: Arc<dyn Remote>,
    projects: RwLock<Vec<ProjectEntry<Q>>>,
    /// Clone + index jobs run one at a time: indexing a large repository is memory heavy.
    index_slot: tokio::sync::Mutex<()>,
    refresh_slot: tokio::sync::Mutex<()>,
}

impl<Q: GraphQueries> AppState<Q> {
    pub fn new(
        data_dir: PathBuf,
        indexer: Indexer,
        config: RuntimeConfig,
        git: GitOptions,
    ) -> Arc<Self> {
        Self::with_remote(data_dir, indexer, config, git, Arc::new(GitRemote))
    }

    pub fn with_remote(
        data_dir: PathBuf,
        indexer: Indexer,
        config: RuntimeConfig,
        git: GitOptions,
        remote: Arc<dyn Remote>,
    ) -> Arc<Self> {
        Arc::new(Self {
            data_dir,
            git,
            config,
            indexer,
            remote,
            projects: RwLock::new(Vec::new()),
            index_slot: tokio::sync::Mutex::new(()),
            refresh_slot: tokio::sync::Mutex::new(()),
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
                    let stale = graph.meta.indexer != hoshizora_core::INDEXER_VERSION;
                    project.status = ProjectStatus::Ready;
                    project.error = None;
                    let state = ProjectState::new(Q::from_graph(graph), root, self.config.clone());
                    fill_counts(&mut project, &state);
                    let id = project.id.clone();
                    self.write().push(ProjectEntry {
                        project,
                        state: Some(state),
                    });
                    if stale {
                        tracing::info!(
                            project = id,
                            "graph built by another hoshizora version, reindexing"
                        );
                        let _ = self.reindex(&id);
                    }
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

    /// Registers a git project and starts cloning + indexing it. Declaring a URL that is
    /// already registered merges the declared name, services and ref into it (a changed ref
    /// queues a rebuild) and leaves its clone and graph alone.
    pub fn add_git(self: &Arc<Self>, req: AddProject) -> Result<Project, AppError> {
        let url = req.url.trim().to_string();
        git::validate_url(&url, self.git.allow_file_urls).map_err(AppError::bad_request)?;
        let declared_ref = req.r#ref.filter(|r| !r.is_empty());
        if let Some(r) = declared_ref.as_deref() {
            git::validate_ref(r).map_err(AppError::bad_request)?;
        }
        let name = req.name.filter(|n| !n.trim().is_empty());
        let merged = {
            let mut projects = self.write();
            let entry = projects.iter_mut().find(|e| match &e.project.source {
                ProjectSource::Git { url: u, .. } => {
                    u.trim_end_matches('/') == url.trim_end_matches('/')
                }
                ProjectSource::Path { .. } => false,
            });
            entry.map(|e| {
                let mut changed = false;
                let mut ref_changed = false;
                if let Some(n) = &name
                    && e.project.name != *n
                {
                    e.project.name = n.clone();
                    changed = true;
                }
                if !req.services.is_empty() && e.project.services != req.services {
                    e.project.services = req.services.clone();
                    changed = true;
                }
                if let ProjectSource::Git { r#ref, .. } = &mut e.project.source
                    && *r#ref != declared_ref
                {
                    *r#ref = declared_ref.clone();
                    e.project.tracked_ref = None;
                    changed = true;
                    ref_changed = true;
                }
                (e.project.clone(), changed, ref_changed)
            })
        };
        if let Some((existing, changed, ref_changed)) = merged {
            if changed {
                self.persist();
            }
            if ref_changed {
                tracing::info!(project = existing.id, "declared ref changed, reindexing");
                return Ok(self.reindex(&existing.id).unwrap_or(existing));
            }
            return Ok(existing);
        }
        let id = self.unique_id(&git::slug_for_url(&url));
        let project = Project {
            name: name.unwrap_or_else(|| id.clone()),
            id: id.clone(),
            source: ProjectSource::Git {
                url,
                r#ref: declared_ref,
            },
            status: ProjectStatus::Cloning,
            error: None,
            commit: None,
            indexed_at: None,
            nodes: 0,
            edges: 0,
            services: req.services,
            tracked_ref: None,
            indexed_ref: None,
            seen_version: None,
            auto_refresh: false,
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
        let existing = {
            let mut projects = self.write();
            let entry = projects.iter_mut().find(|e| match &e.project.source {
                ProjectSource::Path { path: p } => Path::new(p) == path,
                ProjectSource::Git { .. } => false,
            });
            entry.map(|e| {
                let renamed = name.as_ref().is_some_and(|n| e.project.name != *n);
                if let (true, Some(n)) = (renamed, &name) {
                    e.project.name = n.clone();
                }
                (e.project.clone(), renamed)
            })
        };
        if let Some((existing, renamed)) = existing {
            if renamed {
                self.persist();
            }
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
            tracked_ref: None,
            indexed_ref: None,
            seen_version: None,
            auto_refresh: false,
        };
        self.write().push(ProjectEntry {
            project: project.clone(),
            state: None,
        });
        self.persist();
        self.spawn_job(id);
        Ok(project)
    }

    /// Records the `service.version` spans routed to `id` carry; a new version queues a remote
    /// check so a git project without a declared ref can move to the matching tag.
    pub fn note_version(self: &Arc<Self>, id: &str, version: &str) {
        let changed = {
            let mut projects = self.write();
            match projects.iter_mut().find(|e| e.project.id == id) {
                Some(e) if e.project.seen_version.as_deref() != Some(version) => {
                    e.project.seen_version = Some(version.to_string());
                    true
                }
                _ => false,
            }
        };
        if !changed {
            return;
        }
        self.persist();
        let state = Arc::clone(self);
        let id = id.to_string();
        tokio::spawn(async move {
            if let Err(e) = state.refresh_project(&id).await {
                tracing::warn!(project = id, error = %e, "checking remote for new version");
            }
        });
    }

    /// Checks the remote of a git project: adopts the tag matching `seen_version` when the
    /// project has no declared ref, then queues a rebuild if the tracked ref no longer points
    /// at the indexed commit. Checks run one at a time.
    pub async fn refresh_project(self: &Arc<Self>, id: &str) -> anyhow::Result<Refresh> {
        let _slot = self.refresh_slot.lock().await;
        let project = self.get(id).context("project was removed")?;
        let ProjectSource::Git {
            url,
            r#ref: declared,
        } = &project.source
        else {
            return Ok(Refresh::Skipped);
        };
        if busy(project.status) {
            return Ok(Refresh::Skipped);
        }
        if declared.is_none()
            && let Some(version) = &project.seen_version
        {
            let tags = version_tags(version);
            if !tags
                .iter()
                .any(|t| project.tracked_ref.as_deref() == Some(t))
            {
                for tag in &tags {
                    let Some(commit) = self.remote.ls_remote(&self.git, url, tag).await? else {
                        continue;
                    };
                    tracing::info!(project = id, tag, version, "following deployed version");
                    self.set_tracked_ref(id, Some(tag.clone()));
                    if project.commit.as_deref() == Some(commit.as_str()) {
                        return Ok(Refresh::Unchanged);
                    }
                    self.reindex(id)
                        .map_err(|e| anyhow::anyhow!("{}", e.message))?;
                    return Ok(Refresh::Reindexing);
                }
            }
        }
        let tracked = project
            .tracked_ref
            .as_deref()
            .or(declared.as_deref())
            .unwrap_or("HEAD");
        let commit = self
            .remote
            .ls_remote(&self.git, url, tracked)
            .await?
            .with_context(|| format!("remote has no ref {tracked}"))?;
        if project.commit.as_deref() == Some(commit.as_str()) {
            return Ok(Refresh::Unchanged);
        }
        tracing::info!(
            project = id,
            r#ref = tracked,
            commit,
            "ref moved, reindexing"
        );
        self.reindex(id)
            .map_err(|e| anyhow::anyhow!("{}", e.message))?;
        Ok(Refresh::Reindexing)
    }

    pub async fn refresh_all(self: &Arc<Self>) {
        let ids: Vec<String> = self
            .read()
            .iter()
            .filter(|e| matches!(e.project.source, ProjectSource::Git { .. }))
            .filter(|e| e.project.auto_refresh)
            .map(|e| e.project.id.clone())
            .collect();
        for id in ids {
            if let Err(e) = self.refresh_project(&id).await {
                tracing::warn!(project = id, error = %e, "checking remote");
            }
        }
    }

    /// Checks the remote of every git project with `auto_refresh` every `interval`.
    pub fn spawn_refresh(self: &Arc<Self>, interval: Duration) {
        let state = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(interval).await;
                state.refresh_all().await;
            }
        });
    }

    pub fn update(&self, id: &str, req: UpdateProject) -> Result<Project, AppError> {
        let project = {
            let mut projects = self.write();
            let entry = projects
                .iter_mut()
                .find(|e| e.project.id == id)
                .ok_or_else(|| AppError::not_found("project"))?;
            if let Some(on) = req.auto_refresh {
                if on && matches!(entry.project.source, ProjectSource::Path { .. }) {
                    return Err(AppError::bad_request(
                        "a path project has no remote to refresh from",
                    ));
                }
                entry.project.auto_refresh = on;
            }
            entry.project.clone()
        };
        self.persist();
        Ok(project)
    }

    fn set_tracked_ref(&self, id: &str, r#ref: Option<String>) {
        if let Some(entry) = self.write().iter_mut().find(|e| e.project.id == id) {
            entry.project.tracked_ref = r#ref;
        }
        self.persist();
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
        let _slot = self.index_slot.lock().await;
        let project = self.get(id).context("project was removed")?;
        let root = self.root_of(&project);
        let mut commit = None;
        let mut indexed_ref = None;
        if let ProjectSource::Git { url, r#ref } = &project.source {
            let r#ref = project.tracked_ref.as_deref().or(r#ref.as_deref());
            self.set_status(id, ProjectStatus::Cloning, None);
            if root.join(".git").exists() {
                git::update(&self.git, url, r#ref, &root).await?;
            } else {
                let _ = std::fs::remove_dir_all(&root);
                git::clone(&self.git, url, r#ref, &root).await?;
            }
            commit = Some(git::head(&self.git, &root).await?);
            indexed_ref = Some(r#ref.unwrap_or("HEAD").to_string());
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
            let graph = indexer(&index_root, &graph_path)?;
            Ok(ProjectState::new(Q::from_graph(graph), index_root, config))
        })
        .await
        .context("index task panicked")??;
        if let Ok(previous) = self.project_state(Some(id)) {
            previous.persist_stats(&crate::server::stats_path(&self.data_dir, id));
        }
        let mut projects = self.write();
        let entry = projects
            .iter_mut()
            .find(|e| e.project.id == id)
            .context("project was removed")?;
        entry.project.status = ProjectStatus::Ready;
        entry.project.error = None;
        entry.project.commit = commit;
        entry.project.indexed_ref = indexed_ref;
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

/// Tags a deployed `service.version` may be published under, most conventional first.
pub fn version_tags(version: &str) -> Vec<String> {
    let version = version.trim();
    let bare = version.trim_start_matches('v');
    if git::validate_ref(bare).is_err() {
        return Vec::new();
    }
    vec![format!("v{bare}"), bare.to_string()]
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

        let indexer = in_process(|root| crate::build_graph(root, None));
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
        let opts = GitOptions {
            allow_file_urls: true,
            ..Default::default()
        };
        let remote_main = git::ls_remote(&opts, &url, "main").await.unwrap();
        assert_eq!(
            remote_main,
            git::ls_remote(&opts, &url, "HEAD").await.unwrap()
        );
        assert!(remote_main.is_some_and(|sha| sha.len() == 40));
        assert_eq!(git::ls_remote(&opts, &url, "nope").await.unwrap(), None);
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
        assert!(
            restarted
                .list()
                .iter()
                .all(|p| p.status == ProjectStatus::Ready),
            "same-version graphs are not rebuilt"
        );
        for spec in specs {
            restarted.register(spec).unwrap();
        }
        assert_eq!(restarted.list().len(), 2);

        let ids: Vec<String> = restarted.list().into_iter().map(|p| p.id).collect();
        for id in &ids {
            let path = restarted.graph_path(id);
            let mut graph = hoshizora_core::load(&path).unwrap();
            assert_eq!(graph.meta.indexer, hoshizora_core::INDEXER_VERSION);
            graph.meta.indexer = "0.0.0".into();
            std::fs::write(&path, serde_json::to_vec(&graph).unwrap()).unwrap();
        }
        let upgraded = make_state();
        assert_eq!(upgraded.restore().unwrap(), 2);
        assert!(
            upgraded
                .list()
                .iter()
                .all(|p| p.status != ProjectStatus::Ready),
            "graphs from another version are rebuilt"
        );
        assert!(upgraded.get(&ids[0]).is_some());
        for _ in 0..400 {
            if upgraded
                .list()
                .iter()
                .all(|p| matches!(p.status, ProjectStatus::Ready | ProjectStatus::Failed))
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        for id in &ids {
            let graph = hoshizora_core::load(&upgraded.graph_path(id)).unwrap();
            assert_eq!(graph.meta.indexer, hoshizora_core::INDEXER_VERSION);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    struct FakeRemote {
        refs: std::sync::Mutex<std::collections::HashMap<String, String>>,
        calls: std::sync::atomic::AtomicUsize,
    }

    impl FakeRemote {
        fn new(refs: &[(&str, &str)]) -> Arc<Self> {
            Arc::new(Self {
                refs: std::sync::Mutex::new(
                    refs.iter()
                        .map(|(k, v)| (k.to_string(), v.to_string()))
                        .collect(),
                ),
                calls: std::sync::atomic::AtomicUsize::new(0),
            })
        }

        fn set(&self, r#ref: &str, commit: &str) {
            self.refs
                .lock()
                .unwrap()
                .insert(r#ref.into(), commit.into());
        }

        fn calls(&self) -> usize {
            self.calls.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    impl Remote for FakeRemote {
        fn ls_remote<'a>(
            &'a self,
            _git: &'a GitOptions,
            _url: &'a str,
            r#ref: &'a str,
        ) -> Pin<Box<dyn Future<Output = anyhow::Result<Option<String>>> + Send + 'a>> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let found = self.refs.lock().unwrap().get(r#ref).cloned();
            Box::pin(async move { Ok(found) })
        }
    }

    fn fake_state(remote: Arc<FakeRemote>) -> Arc<AppState<crate::queries::GraphIndex>> {
        let dir = std::env::temp_dir().join(format!(
            "hoshizora-remote-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        AppState::with_remote(
            dir,
            in_process(|_| Ok(Graph::default())),
            RuntimeConfig::default(),
            GitOptions {
                allow_file_urls: true,
                ..Default::default()
            },
            remote,
        )
    }

    /// A ready git project at `commit`, inserted without cloning anything.
    fn ready_git(
        state: &Arc<AppState<crate::queries::GraphIndex>>,
        url: &str,
        r#ref: Option<&str>,
        commit: &str,
    ) -> Project {
        let id = git::slug_for_url(url);
        let project = Project {
            name: id.clone(),
            id: id.clone(),
            source: ProjectSource::Git {
                url: url.into(),
                r#ref: r#ref.map(Into::into),
            },
            status: ProjectStatus::Ready,
            error: None,
            commit: Some(commit.into()),
            indexed_at: None,
            nodes: 0,
            edges: 0,
            services: Vec::new(),
            tracked_ref: None,
            indexed_ref: None,
            seen_version: None,
            auto_refresh: false,
        };
        let root = state.repo_dir(&id);
        state.insert_ready(
            project,
            crate::queries::GraphIndex::new(Graph::default()),
            root,
            None,
        )
    }

    #[tokio::test]
    async fn declared_metadata_merges_into_existing_registration() {
        let state = fake_state(FakeRemote::new(&[]));
        let url = "file:///nowhere/a/b";
        let existing = ready_git(&state, url, None, "abc");
        assert!(existing.services.is_empty());

        let merged = state
            .add_git(AddProject {
                url: url.into(),
                r#ref: None,
                name: Some("renamed".into()),
                services: vec!["b".into()],
            })
            .unwrap();
        assert_eq!(merged.id, existing.id);
        assert_eq!(merged.name, "renamed");
        assert_eq!(merged.services, vec!["b".to_string()]);
        assert_eq!(merged.status, ProjectStatus::Ready, "same ref: no rebuild");
        assert_eq!(merged.commit.as_deref(), Some("abc"));

        let kept = state
            .add_git(AddProject {
                url: url.into(),
                r#ref: None,
                name: None,
                services: Vec::new(),
            })
            .unwrap();
        assert_eq!(
            kept.services,
            vec!["b".to_string()],
            "empty declaration keeps services"
        );
        assert_eq!(kept.name, "renamed");

        let saved: Vec<Project> =
            serde_json::from_slice(&std::fs::read(state.data_dir.join("projects.json")).unwrap())
                .unwrap();
        assert_eq!(saved[0].services, vec!["b".to_string()]);

        let repinned = state
            .add_git(AddProject {
                url: url.into(),
                r#ref: Some("v2".into()),
                name: None,
                services: Vec::new(),
            })
            .unwrap();
        assert!(matches!(&repinned.source, ProjectSource::Git { r#ref: Some(r), .. } if r == "v2"));
        assert_eq!(
            repinned.status,
            ProjectStatus::Cloning,
            "new ref queues a rebuild"
        );
        assert_eq!(state.list().len(), 1);
        let _ = std::fs::remove_dir_all(&state.data_dir);
    }

    #[tokio::test]
    async fn periodic_check_skips_projects_not_opted_in() {
        let remote = FakeRemote::new(&[("HEAD", "moved")]);
        let state = fake_state(Arc::clone(&remote));
        let off = ready_git(&state, "file:///nowhere/p/off", None, "abc");
        let on = ready_git(&state, "file:///nowhere/p/on", None, "abc");

        state.refresh_all().await;
        assert_eq!(remote.calls(), 0, "nothing is checked by default");
        assert!(
            state
                .list()
                .iter()
                .all(|p| p.status == ProjectStatus::Ready)
        );

        state
            .update(
                &on.id,
                UpdateProject {
                    auto_refresh: Some(true),
                },
            )
            .unwrap();
        state.refresh_all().await;
        assert_eq!(remote.calls(), 1);
        assert_eq!(state.get(&on.id).unwrap().status, ProjectStatus::Cloning);
        assert_eq!(state.get(&off.id).unwrap().status, ProjectStatus::Ready);
        let _ = std::fs::remove_dir_all(&state.data_dir);
    }

    #[tokio::test]
    async fn remote_check_reindexes_only_when_the_commit_moved() {
        let remote = FakeRemote::new(&[("HEAD", "abc"), ("main", "m1")]);
        let state = fake_state(Arc::clone(&remote));
        let unpinned = ready_git(&state, "file:///nowhere/x/unpinned", None, "abc");
        let pinned = ready_git(&state, "file:///nowhere/x/pinned", Some("main"), "m1");

        assert_eq!(
            state.refresh_project(&unpinned.id).await.unwrap(),
            Refresh::Unchanged
        );
        assert_eq!(
            state.refresh_project(&pinned.id).await.unwrap(),
            Refresh::Unchanged
        );
        assert_eq!(remote.calls(), 2);
        assert!(
            state
                .list()
                .iter()
                .all(|p| p.status == ProjectStatus::Ready)
        );

        remote.set("main", "m2");
        assert_eq!(
            state.refresh_project(&pinned.id).await.unwrap(),
            Refresh::Reindexing
        );
        assert_eq!(
            state.get(&pinned.id).unwrap().status,
            ProjectStatus::Cloning
        );
        assert_eq!(
            state.refresh_project(&pinned.id).await.unwrap(),
            Refresh::Skipped,
            "a project being rebuilt is not checked again"
        );
        assert_eq!(
            state.get(&unpinned.id).unwrap().status,
            ProjectStatus::Ready
        );
        let _ = std::fs::remove_dir_all(&state.data_dir);
    }

    #[tokio::test]
    async fn seen_version_moves_an_unpinned_project_to_its_tag() {
        let remote = FakeRemote::new(&[
            ("HEAD", "abc"),
            ("v1.2.0", "t1"),
            ("2.0.0", "t2"),
            ("main", "abc"),
        ]);
        let state = fake_state(Arc::clone(&remote));
        let vee = ready_git(&state, "file:///nowhere/t/vee", None, "abc");
        let bare = ready_git(&state, "file:///nowhere/t/bare", None, "abc");
        let missing = ready_git(&state, "file:///nowhere/t/missing", None, "abc");
        let pinned = ready_git(&state, "file:///nowhere/t/pinned", Some("main"), "abc");
        for (p, v) in [
            (&vee, "1.2.0"),
            (&bare, "2.0.0"),
            (&missing, "9.9.9"),
            (&pinned, "1.2.0"),
        ] {
            state
                .write()
                .iter_mut()
                .find(|e| e.project.id == p.id)
                .unwrap()
                .project
                .seen_version = Some(v.into());
        }

        assert_eq!(
            state.refresh_project(&vee.id).await.unwrap(),
            Refresh::Reindexing
        );
        let vee = state.get(&vee.id).unwrap();
        assert_eq!(vee.tracked_ref.as_deref(), Some("v1.2.0"));
        assert_eq!(vee.status, ProjectStatus::Cloning);

        assert_eq!(
            state.refresh_project(&bare.id).await.unwrap(),
            Refresh::Reindexing
        );
        assert_eq!(
            state.get(&bare.id).unwrap().tracked_ref.as_deref(),
            Some("2.0.0")
        );

        assert_eq!(
            state.refresh_project(&missing.id).await.unwrap(),
            Refresh::Unchanged,
            "no tag for the version: keep following HEAD"
        );
        assert_eq!(state.get(&missing.id).unwrap().tracked_ref, None);

        assert_eq!(
            state.refresh_project(&pinned.id).await.unwrap(),
            Refresh::Unchanged
        );
        assert_eq!(
            state.get(&pinned.id).unwrap().tracked_ref,
            None,
            "a declared ref is never overridden"
        );

        let saved: Vec<Project> =
            serde_json::from_slice(&std::fs::read(state.data_dir.join("projects.json")).unwrap())
                .unwrap();
        assert_eq!(saved[0].seen_version.as_deref(), Some("1.2.0"));
        assert_eq!(saved[0].tracked_ref.as_deref(), Some("v1.2.0"));
        let _ = std::fs::remove_dir_all(&state.data_dir);
    }

    #[test]
    fn version_tag_candidates() {
        assert_eq!(version_tags("1.2.0"), vec!["v1.2.0", "1.2.0"]);
        assert_eq!(
            version_tags("v0.29.0-beta.1"),
            vec!["v0.29.0-beta.1", "0.29.0-beta.1"]
        );
        assert!(version_tags("--bad").is_empty());
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
