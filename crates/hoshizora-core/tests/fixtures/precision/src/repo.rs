use sqlx::PgPool;

pub struct Repo {
    pool: PgPool,
}

impl Repo {
    pub async fn find(&self, id: i64) -> i64 {
        sqlx::query("SELECT * FROM items WHERE id = $1").bind(id).fetch_one(&self.pool).await.unwrap();
        id
    }

    pub async fn find_all(&self, ids: &[i64]) -> Vec<i64> {
        let mut out = Vec::new();
        for id in ids {
            out.push(self.find(*id).await);
        }
        out
    }
}

pub async fn warm(repo: &Repo, ids: &[i64]) {
    for id in ids {
        repo.find(*id).await;
    }
}

pub struct IdService {
    pool: PgPool,
}

impl IdService {
    pub async fn assign(&self, id: i64) -> i64 {
        sqlx::query("INSERT INTO arr_ids (entity) VALUES ($1)").bind(id).execute(&self.pool).await.unwrap();
        id
    }
}

pub struct Services {
    pub ids: IdService,
}

pub struct State {
    pub services: Services,
}

pub async fn to_id(state: &State, id: i64) -> i64 {
    state.services.ids.assign(id).await
}

pub async fn sync(state: &State, ids: &[i64]) -> Vec<i64> {
    let mut out = Vec::new();
    for id in ids {
        out.push(to_id(state, *id).await);
    }
    out
}
