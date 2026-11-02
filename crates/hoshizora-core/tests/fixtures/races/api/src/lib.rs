use sqlx::PgPool;

pub async fn claim(pool: &PgPool, id: i64) -> bool {
    let job = sqlx::query("SELECT status FROM jobs WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await;
    if job.is_ok() {
        sqlx::query("UPDATE jobs SET status = 'running' WHERE id = $1")
            .bind(id)
            .execute(pool)
            .await
            .ok();
    }
    true
}

pub async fn claim_in_tx(pool: &PgPool, id: i64) {
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SELECT status FROM jobs WHERE id = $1")
        .bind(id)
        .fetch_one(&mut *tx)
        .await
        .ok();
    sqlx::query("UPDATE jobs SET status = 'running' WHERE id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await
        .ok();
    tx.commit().await.ok();
}

pub async fn claim_locked(pool: &PgPool, id: i64) {
    sqlx::query("SELECT status FROM jobs WHERE id = $1 FOR UPDATE")
        .bind(id)
        .fetch_one(pool)
        .await
        .ok();
    sqlx::query("UPDATE jobs SET status = 'running' WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await
        .ok();
}

pub async fn purge(pool: &PgPool) {
    let n = count_jobs(pool).await;
    if n > 10 {
        delete_old(pool).await;
    }
}

async fn count_jobs(pool: &PgPool) -> i64 {
    sqlx::query("SELECT count(*) FROM jobs").fetch_one(pool).await.ok();
    0
}

async fn delete_old(pool: &PgPool) {
    sqlx::query("DELETE FROM jobs WHERE status = 'done'")
        .execute(pool)
        .await
        .ok();
}

pub async fn unrelated(pool: &PgPool) {
    sqlx::query("SELECT id FROM users").fetch_all(pool).await.ok();
    sqlx::query("UPDATE jobs SET status = 'failed'").execute(pool).await.ok();
}

pub async fn write_then_read(pool: &PgPool) {
    sqlx::query("UPDATE jobs SET status = 'queued'").execute(pool).await.ok();
    sqlx::query("SELECT id FROM jobs").fetch_all(pool).await.ok();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn claims_twice(pool: &PgPool) {
        sqlx::query("SELECT status FROM jobs").fetch_one(pool).await.ok();
        sqlx::query("UPDATE jobs SET status = 'running'").execute(pool).await.ok();
    }
}
