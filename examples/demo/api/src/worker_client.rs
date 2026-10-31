use serde::{Deserialize, Serialize};
use tracing::instrument;

use crate::db::Star;
use crate::telemetry::inject_traceparent;
use crate::AppState;

#[derive(Debug, Deserialize, Serialize)]
pub struct Weather {
    pub site: String,
    pub sky: String,
    pub seeing: f64,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Magnitudes {
    pub magnitudes: Vec<StarMagnitude>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct StarMagnitude {
    pub id: i32,
    pub magnitude: f64,
}

#[instrument(skip(state), fields(otel.kind = "client", http.request.method = "GET", server.address = %state.worker_url, url.path = "/weather"))]
pub async fn fetch_weather(state: &AppState, site: &str) -> anyhow::Result<Weather> {
    let mut headers = http::HeaderMap::new();
    inject_traceparent(&mut headers);
    let res = state
        .http
        .get(format!("{}/weather/{site}", state.worker_url))
        .headers(headers)
        .send()
        .await?
        .error_for_status()?;
    Ok(res.json().await?)
}

#[instrument(skip(state, stars), fields(otel.kind = "client", http.request.method = "POST", server.address = %state.worker_url, url.path = "/compute/magnitudes", star_count = stars.len()))]
pub async fn compute_magnitudes(state: &AppState, stars: &[Star]) -> anyhow::Result<Magnitudes> {
    let mut headers = http::HeaderMap::new();
    inject_traceparent(&mut headers);
    let res = state
        .http
        .post(format!("{}/compute/magnitudes", state.worker_url))
        .headers(headers)
        .json(&serde_json::json!({ "stars": stars }))
        .send()
        .await?
        .error_for_status()?;
    Ok(res.json().await?)
}
