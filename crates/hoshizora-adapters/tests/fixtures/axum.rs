//! Session HTTP handlers.

use axum::{Json, extract::State};
use sqlx::PgPool;
use crate::model::{Session, User as U};

/// Stored session.
#[derive(Debug, Clone)]
pub struct SessionStore {
    pool: PgPool,
}

pub trait Store {
    /// Fetch one.
    fn get(&self, id: &str) -> Option<Session>;
    fn describe(&self) -> String {
        format!("store")
    }
}

impl Store for SessionStore {
    fn get(&self, id: &str) -> Option<Session> {
        None
    }
}

impl SessionStore {
    /// Loads every session and refreshes the owners.
    ///
    /// Runs one query per user.
    #[tracing::instrument(skip(self))]
    pub async fn refresh(&self, user_ids: &[i64], limit: Option<u32>) -> anyhow::Result<Vec<Session>> {
        let mut out = Vec::new();
        for id in user_ids {
            let row = sqlx::query_as!(Session, "SELECT * FROM sessions WHERE user_id = $1", id)
                .fetch_one(&self.pool)
                .await?;
            if row.active {
                out.push(row);
            } else {
                tracing::warn!("inactive {}", id);
            }
        }
        let handle = tokio::spawn(async move {
            reqwest::get("https://example.com/ping").await
        });
        let names: Vec<_> = out.iter().map(|s| s.name.clone()).collect();
        match limit {
            Some(n) if n > 0 => out.truncate(n as usize),
            _ => {}
        }
        Ok(out)
    }
}

/// GET /sessions
pub async fn list(State(store): State<SessionStore>) -> Json<Vec<Session>> {
    let sessions = store.refresh(&[1, 2], None).await.unwrap_or_default();
    Json(sessions)
}

#[tokio::main]
async fn main() {
    let app = axum::Router::new().route("/sessions", axum::routing::get(list));
    let _ = app;
}

fn router(client: reqwest::Client, base: &str) -> axum::Router {
    let weather = client.get(format!("{}/weather/{site}", base));
    let label = client.get(concat!("a", "b"));
    let _ = (weather, label);
    axum::Router::new()
        .route("/stars", get(list_stars))
        .route("/stars/{id}", get(get_star).delete(delete_star))
}

pub const MAX: usize = 10;

#[derive(Deserialize)]
pub struct Observation {
    #[serde(default = "default_site")]
    pub site: String,
    #[serde(rename = "mag")]
    pub magnitude: f64,
}

pub async fn stats(p: PgPool, mut rx: Receiver<u8>) -> (i64, i64) {
    let (stars, obs) = tokio::join!(
        db::count_stars(&p),
        db::count_observations(&p, "SELECT count(*) FROM observations"),
    );
    let names = vec![first(stars), second(obs)];
    tokio::select! {
        Some(v) = rx.recv() => { handle(v).await; }
        _ = tokio::time::sleep(Duration::from_secs(1)) => {}
    }
    println!("{} {}", names.len(), fmt_count(obs));
    macro_rules! ignored { () => { skipped() }; }
    (stars, obs)
}
