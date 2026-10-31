use serde::Serialize;
use sqlx::{FromRow, PgPool};
use tracing::instrument;

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Star {
    pub id: i32,
    pub name: String,
    pub constellation: String,
    pub distance_ly: f64,
    pub flux: f64,
    pub magnitude: Option<f64>,
}

#[derive(Debug, Serialize, FromRow)]
pub struct Instrument {
    pub instrument: String,
    pub available: i32,
}

#[instrument(skip(pool), fields(db.system.name = "postgresql", db.operation.name = "SELECT", db.collection.name = "stars", db.query.text = "SELECT id, name, constellation, distance_ly, flux, magnitude FROM stars ORDER BY id"))]
pub async fn all_stars(pool: &PgPool) -> sqlx::Result<Vec<Star>> {
    sqlx::query_as(
        "SELECT id, name, constellation, distance_ly, flux, magnitude FROM stars ORDER BY id",
    )
    .fetch_all(pool)
    .await
}

#[instrument(skip(pool), fields(db.system.name = "postgresql", db.operation.name = "SELECT", db.collection.name = "observations", db.query.text = "SELECT count(*) FROM observations WHERE star_id = $1"))]
pub async fn observation_count_for_star(pool: &PgPool, star_id: i32) -> sqlx::Result<i64> {
    sqlx::query_scalar("SELECT count(*) FROM observations WHERE star_id = $1")
        .bind(star_id)
        .fetch_one(pool)
        .await
}

#[instrument(skip(pool), fields(db.system.name = "postgresql", db.operation.name = "SELECT", db.collection.name = "stars", db.query.text = "SELECT id, name, constellation, distance_ly, flux, magnitude FROM stars WHERE name = $1"))]
pub async fn star_by_name(pool: &PgPool, name: &str) -> sqlx::Result<Option<Star>> {
    sqlx::query_as(
        "SELECT id, name, constellation, distance_ly, flux, magnitude FROM stars WHERE name = $1",
    )
    .bind(name)
    .fetch_optional(pool)
    .await
}

#[instrument(skip(pool), fields(db.system.name = "postgresql", db.operation.name = "SELECT", db.collection.name = "inventory", db.query.text = "SELECT instrument, available FROM inventory"))]
pub async fn inventory(pool: &PgPool) -> sqlx::Result<Vec<Instrument>> {
    sqlx::query_as("SELECT instrument, available FROM inventory")
        .fetch_all(pool)
        .await
}

#[instrument(skip(pool), fields(db.system.name = "postgresql", db.operation.name = "INSERT", db.collection.name = "observations", db.query.text = "INSERT INTO observations (star_id, site, seeing) VALUES ($1, $2, $3) RETURNING id"))]
pub async fn insert_observation(
    pool: &PgPool,
    star_id: i32,
    site: &str,
    seeing: f64,
) -> sqlx::Result<i32> {
    sqlx::query_scalar(
        "INSERT INTO observations (star_id, site, seeing) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(star_id)
    .bind(site)
    .bind(seeing)
    .fetch_one(pool)
    .await
}

#[instrument(skip(pool, payload), fields(db.system.name = "postgresql", db.operation.name = "INSERT", db.collection.name = "events", db.query.text = "INSERT INTO events (topic, payload) VALUES ($1, $2)"))]
pub async fn insert_event(
    pool: &PgPool,
    topic: &str,
    payload: serde_json::Value,
) -> sqlx::Result<()> {
    sqlx::query("INSERT INTO events (topic, payload) VALUES ($1, $2)")
        .bind(topic)
        .bind(payload)
        .execute(pool)
        .await?;
    Ok(())
}

#[instrument(skip(pool), fields(db.system.name = "postgresql", db.operation.name = "UPDATE", db.collection.name = "stars", db.query.text = "UPDATE stars SET magnitude = $2 WHERE id = $1"))]
pub async fn update_magnitude(pool: &PgPool, star_id: i32, magnitude: f64) -> sqlx::Result<()> {
    sqlx::query("UPDATE stars SET magnitude = $2 WHERE id = $1")
        .bind(star_id)
        .bind(magnitude)
        .execute(pool)
        .await?;
    Ok(())
}

#[instrument(skip(pool), fields(db.system.name = "postgresql", db.operation.name = "SELECT", db.collection.name = "stars", db.query.text = "SELECT count(*) FROM stars"))]
pub async fn count_stars(pool: &PgPool) -> sqlx::Result<i64> {
    sqlx::query_scalar("SELECT count(*) FROM stars")
        .fetch_one(pool)
        .await
}

#[instrument(skip(pool), fields(db.system.name = "postgresql", db.operation.name = "SELECT", db.collection.name = "observations", db.query.text = "SELECT count(*) FROM observations"))]
pub async fn count_observations(pool: &PgPool) -> sqlx::Result<i64> {
    sqlx::query_scalar("SELECT count(*) FROM observations")
        .fetch_one(pool)
        .await
}
