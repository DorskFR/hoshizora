//! The project registry: `projects.json`, clones under `repos/`, graphs under `graphs/`, and the
//! background clone + index jobs that move a project to `ready`.

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};

use anyhow::Context;
use hoshizora_model::Graph;
use hoshizora_model::api::{AddProject, Project, ProjectSource, ProjectStatus};
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
            let loaded = (graph_path.exists() && root.exists())
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

    #[test]
    fn url_or_path() {
        assert!(is_url("https://github.com/a/b"));
        assert!(is_url("git@github.com:a/b"));
        assert!(!is_url("./src"));
        assert!(!is_url("/tmp/repo"));
        assert_eq!(path_slug(Path::new("/tmp/My Repo")), "my-repo");
    }
}
