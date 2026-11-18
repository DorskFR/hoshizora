pub async fn get_or_assign(pool: &sqlx::PgPool, key: &str) -> i64 {
    let found: Option<i64> = sqlx::query_scalar("SELECT id FROM arr_ids WHERE key = $1").bind(key).fetch_optional(pool).await.unwrap();
    if let Some(id) = found {
        return id;
    }
    sqlx::query("INSERT INTO arr_ids (key) VALUES ($1) ON CONFLICT (key) DO NOTHING").bind(key).execute(pool).await.unwrap();
    0
}

pub async fn rename(pool: &sqlx::PgPool, key: &str) {
    let found: Option<i64> = sqlx::query_scalar("SELECT id FROM labels WHERE key = $1").bind(key).fetch_optional(pool).await.unwrap();
    if found.is_none() {
        sqlx::query("INSERT INTO labels (key) VALUES ($1)").bind(key).execute(pool).await.unwrap();
    }
}

pub struct Row {
    pub id: i64,
    pub hits: i64,
}

pub async fn find(pool: &sqlx::PgPool, id: i64) -> Row {
    let hits: i64 = sqlx::query_scalar("SELECT hits FROM counters WHERE id = $1").bind(id).fetch_one(pool).await.unwrap();
    Row { id, hits }
}

pub async fn save(pool: &sqlx::PgPool, id: i64, hits: i64) {
    sqlx::query("UPDATE counters SET hits = $2 WHERE id = $1").bind(id).bind(hits).execute(pool).await.unwrap();
}

pub async fn bump(pool: &sqlx::PgPool, id: i64) {
    let mut row = find(pool, id).await;
    row.hits += 1;
    save(pool, row.id, row.hits).await;
}

pub async fn close(pool: &sqlx::PgPool, id: i64) {
    let _row = find(pool, id).await;
    save(pool, id, 0).await;
}

pub async fn forget(pool: &sqlx::PgPool, id: i64) {
    let row = find(pool, id).await;
    sqlx::query("DELETE FROM counters WHERE id = $1").bind(row.id).execute(pool).await.unwrap();
}

pub async fn find_all(pool: &sqlx::PgPool) -> Vec<Row> {
    let rows: Vec<(i64, i64)> = sqlx::query_as("SELECT id, hits FROM counters").fetch_all(pool).await.unwrap();
    rows.into_iter().map(|(id, hits)| Row { id, hits }).collect()
}

pub async fn decay(pool: &sqlx::PgPool) {
    let rows = find_all(pool).await;
    for row in &rows {
        save(pool, row.id, row.hits / 2).await;
    }
}

pub async fn rearm(pool: &sqlx::PgPool, id: i64, enabled: bool) -> i64 {
    let row = find(pool, id).await;
    if enabled {
        save(pool, id, 0).await;
    }
    row.hits
}

pub async fn bump_unique(pool: &sqlx::PgPool, id: i64) {
    let row = find(pool, id).await;
    sqlx::query("UPDATE counters SET hits = $2 WHERE id = $1 AND NOT EXISTS (SELECT 1 FROM counters WHERE hits = $2)")
        .bind(id)
        .bind(row.hits + 1)
        .execute(pool)
        .await
        .unwrap();
}
