use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::instrument;

use crate::catalog::{distance_modulus, find_duplicate_stars, warm_lens};
use crate::db::{self, Star};
use crate::worker_client::{compute_magnitudes, fetch_weather};
use crate::AppState;

type ApiError = (StatusCode, String);

fn internal(e: impl std::fmt::Display) -> ApiError {
    tracing::error!(error = %e, "request failed");
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

pub async fn health() -> &'static str {
    "ok"
}

#[derive(Serialize)]
pub struct StarWithObservations {
    #[serde(flatten)]
    pub star: Star,
    pub observations: i64,
}

/// N+1: one query for the list, then one query per star.
#[instrument(skip(state))]
pub async fn list_stars(
    State(state): State<AppState>,
) -> Result<Json<Vec<StarWithObservations>>, ApiError> {
    let stars = db::all_stars(&state.pool).await.map_err(internal)?;
    let mut out = Vec::with_capacity(stars.len());
    for star in stars {
        let observations = db::observation_count_for_star(&state.pool, star.id)
            .await
            .map_err(internal)?;
        out.push(StarWithObservations { star, observations });
    }
    Ok(Json(out))
}

#[derive(Deserialize, Debug)]
pub struct LaunchRequest {
    pub star: String,
    #[serde(default = "default_site")]
    pub site: String,
}

fn default_site() -> String {
    "paranal".into()
}

#[instrument(skip(state))]
pub async fn launch_observation(
    State(state): State<AppState>,
    Json(req): Json<LaunchRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let star = db::star_by_name(&state.pool, &req.star)
        .await
        .map_err(internal)?
        .ok_or((StatusCode::NOT_FOUND, format!("unknown star {}", req.star)))?;

    // Independent, yet awaited one after the other.
    let inventory = db::inventory(&state.pool).await.map_err(internal)?;
    let weather = fetch_weather(&state, &req.site).await.map_err(internal)?;

    if inventory.iter().all(|i| i.available == 0) {
        return Err((StatusCode::CONFLICT, "no instrument available".into()));
    }

    warm_lens();

    let observation_id = db::insert_observation(&state.pool, star.id, &req.site, weather.seeing)
        .await
        .map_err(internal)?;

    let pool = state.pool.clone();
    let payload =
        json!({ "observation_id": observation_id, "star": star.name, "sky": weather.sky });
    tokio::spawn(record_observation_event(pool, payload));

    Ok(Json(json!({
        "observation_id": observation_id,
        "star": star.name,
        "weather": weather,
        "instruments": inventory,
    })))
}

#[instrument(skip(pool, payload))]
async fn record_observation_event(pool: sqlx::PgPool, payload: serde_json::Value) {
    tokio::time::sleep(std::time::Duration::from_millis(15)).await;
    if let Err(e) = db::insert_event(&pool, "observation.launched", payload).await {
        tracing::warn!(error = %e, "could not record event");
    }
}

#[instrument(skip(state))]
pub async fn recompute_catalog(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let stars = db::all_stars(&state.pool).await.map_err(internal)?;
    let duplicates = find_duplicate_stars(&stars);
    let magnitudes = compute_magnitudes(&state, &stars).await.map_err(internal)?;

    for m in &magnitudes.magnitudes {
        db::update_magnitude(&state.pool, m.id, m.magnitude)
            .await
            .map_err(internal)?;
    }

    let updated = cache_moduli(&state, &stars);

    Ok(Json(json!({
        "updated": updated,
        "duplicates": duplicates,
        "magnitudes": magnitudes.magnitudes,
    })))
}

/// Sync point: every recompute serialises on the catalog cache mutex.
#[instrument(skip(state, stars))]
fn cache_moduli(state: &AppState, stars: &[Star]) -> usize {
    let mut cache = state.catalog_cache.lock().unwrap();
    for star in stars {
        cache.insert(star.name.clone(), distance_modulus(star.distance_ly));
    }
    cache.len()
}

#[instrument(skip(state))]
pub async fn catalog_summary(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (stars, observations) = tokio::join!(
        db::count_stars(&state.pool),
        db::count_observations(&state.pool)
    );
    Ok(Json(json!({
        "stars": stars.map_err(internal)?,
        "observations": observations.map_err(internal)?,
    })))
}
