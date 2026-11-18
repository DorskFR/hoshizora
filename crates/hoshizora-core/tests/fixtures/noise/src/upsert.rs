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
