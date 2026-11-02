use sqlx::PgPool;

pub struct Repo {
    pool: PgPool,
}

pub struct Remote {
    base: String,
}

impl Remote {
    pub async fn fetch_weather(&self, city: &str) -> String {
        reqwest::get(format!("{}/{city}", self.base)).await.unwrap().text().await.unwrap()
    }
}

impl Repo {
    pub async fn fetch_ids(&self, sql: &str) -> Vec<i64> {
        sqlx::query(sql).fetch_all(&self.pool).await.ok();
        vec![]
    }

    pub async fn albums(&self, ids: &[i64]) -> Vec<String> {
        sqlx::query("SELECT name FROM albums WHERE id = ANY($1)")
            .bind(ids)
            .fetch_all(&self.pool)
            .await
            .ok();
        vec![]
    }

    pub async fn lookup(&self, tx: &mut sqlx::PgConnection, id: i64) {
        sqlx::query("SELECT 1 FROM a WHERE id = $1").bind(id).fetch_one(tx).await.ok();
    }

    pub async fn insert(&self, tx: &mut sqlx::PgConnection, id: i64) {
        sqlx::query("INSERT INTO b (id) VALUES ($1)").bind(id).execute(tx).await.ok();
    }

    pub async fn mark(&self, ids: Vec<String>) {
        sqlx::query("UPDATE albums SET seen = true WHERE name = ANY($1)")
            .bind(ids)
            .execute(&self.pool)
            .await
            .ok();
    }
}

pub async fn dependent_via_binding(catalog: &Repo, repo: &Repo, sql: &str) -> Vec<String> {
    let ids = catalog.fetch_ids(sql).await;
    repo.albums(&ids).await
}

pub async fn dependent_via_free_function(catalog: &Repo, remote: &Remote, sql: &str) -> String {
    let rows = catalog.fetch_ids(sql).await;
    decorate(remote, &rows).await
}

async fn decorate(remote: &Remote, rows: &[i64]) -> String {
    remote.fetch_weather(&rows.len().to_string()).await
}

pub async fn dependent_via_collect(catalog: &Repo, repo: &Repo, sql: &str) {
    let rows = catalog.fetch_ids(sql).await;
    let names: Vec<String> = rows.into_iter().map(|r| r.to_string()).collect();
    repo.mark(names).await;
}

pub async fn commit_barrier(pool: &PgPool, repo: &Repo, sql: &str) {
    let tx = pool.begin().await.unwrap();
    tx.commit().await.unwrap();
    repo.fetch_ids(sql).await;
}

pub async fn shared_mut_tx(pool: &PgPool, catalog: &Repo, repo: &Repo, id: i64) {
    let mut tx = pool.begin().await.unwrap();
    catalog.lookup(&mut *tx, id).await;
    repo.insert(&mut *tx, id).await;
}

pub async fn independent(repo: &Repo, remote: &Remote, sql: &str, city: &str) -> (Vec<i64>, String) {
    let ids = repo.fetch_ids(sql).await;
    let weather = remote.fetch_weather(city).await;
    (ids, weather)
}
