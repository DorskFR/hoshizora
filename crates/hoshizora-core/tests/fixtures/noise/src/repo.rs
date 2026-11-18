pub async fn load_all(pool: &sqlx::PgPool, ids: &[i64]) -> Vec<String> {
    let mut out = Vec::new();
    for id in ids {
        out.push(sqlx::query("SELECT name FROM users WHERE id = $1").bind(id).fetch_one(pool).await.unwrap());
    }
    out
}

#[cfg(test)]
mod checks {
    async fn seed(pool: &sqlx::PgPool, ids: &[i64]) {
        for id in ids {
            sqlx::query("INSERT INTO users (id) VALUES ($1)").bind(id).execute(pool).await.unwrap();
        }
    }

    #[tokio::test]
    async fn loads_every_user() {
        let pool = sqlx::PgPool::connect("postgres://").await.unwrap();
        let ids: Vec<i64> = (1..100).collect();
        for id in &ids {
            sqlx::query("SELECT name FROM users WHERE id = $1").bind(id).fetch_one(&pool).await.unwrap();
        }
        seed(&pool, &[1]).await;
    }
}
