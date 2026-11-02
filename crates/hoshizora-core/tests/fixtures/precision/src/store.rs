use std::collections::HashMap;

pub struct Store {
    pool: sqlx::PgPool,
}

impl Store {
    pub async fn get(&self, key: &str) -> String {
        sqlx::query("SELECT value FROM settings WHERE key = $1").bind(key).fetch_one(&self.pool).await.unwrap()
    }
}

pub fn build() -> HashMap<String, String> {
    HashMap::new()
}

pub fn lookup(keys: &[String]) -> usize {
    let map = build();
    let mut hits = 0;
    for k in keys {
        if map.get(k).is_some() {
            hits += 1;
        }
    }
    hits
}

pub async fn relay(keys: &[String]) -> Vec<String> {
    let handle = make_handle();
    let mut out = Vec::new();
    for k in keys {
        out.push(handle.dispatch(k).await);
    }
    out
}

fn make_handle() -> crate::scheduler::Dispatcher {
    crate::scheduler::Dispatcher
}
