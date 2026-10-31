mod catalog;
mod db;
mod handlers;
mod telemetry;
mod worker_client;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::http::Method;
use axum::routing::{get, post};
use axum::{middleware, Router};
use sqlx::postgres::PgPoolOptions;
use tower_http::cors::{Any, CorsLayer};

#[derive(Clone)]
pub struct AppState {
    pub pool: sqlx::PgPool,
    pub http: reqwest::Client,
    pub worker_url: String,
    pub catalog_cache: Arc<Mutex<HashMap<String, f64>>>,
}

fn main() -> anyhow::Result<()> {
    let provider = telemetry::init("observatory-api")?;
    tokio::runtime::Runtime::new()?.block_on(run())?;
    provider.shutdown()?;
    Ok(())
}

async fn run() -> anyhow::Result<()> {
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://observatory:observatory@localhost:5432/observatory".into());
    let worker_url = std::env::var("WORKER_URL").unwrap_or_else(|_| "http://localhost:8001".into());
    let bind = std::env::var("BIND").unwrap_or_else(|_| "0.0.0.0:8081".into());

    let pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&database_url)
        .await?;

    let state = AppState {
        pool,
        http: reqwest::Client::new(),
        worker_url,
        catalog_cache: Arc::new(Mutex::new(HashMap::new())),
    };

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
        .allow_headers(Any)
        .expose_headers(Any);

    let app = Router::new()
        .route("/health", get(handlers::health))
        .route("/stars", get(handlers::list_stars))
        .route("/observations", post(handlers::launch_observation))
        .route("/catalog/recompute", post(handlers::recompute_catalog))
        .route("/catalog/summary", get(handlers::catalog_summary))
        .layer(middleware::from_fn(telemetry::http_server_span))
        .layer(cors)
        .with_state(state);

    tracing::info!(%bind, "observatory-api listening");
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
