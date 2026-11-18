async fn reload(pool: &sqlx::PgPool, ids: &[i64]) {
    for id in ids {
        sqlx::query("SELECT name FROM users WHERE id = $1").bind(id).fetch_one(pool).await.unwrap();
    }
}
