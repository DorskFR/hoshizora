pub async fn seed_counters(pool: &sqlx::PgPool, ids: &[i64]) {
    for id in ids {
        sqlx::query("INSERT INTO counters (id) VALUES ($1)").bind(id).execute(pool).await.unwrap();
    }
}
