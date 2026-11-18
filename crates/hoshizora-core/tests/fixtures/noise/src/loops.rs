use std::time::Duration;

const TABLES: &[&str] = &["users", "sessions"];

pub struct Media {
    pub tracks: Vec<Track>,
}

pub struct Track {
    pub parts: Vec<String>,
}

pub async fn retry_fetch(client: &reqwest::Client) -> String {
    for attempt in 0..3 {
        if let Ok(r) = client.get("https://api.example.com/status").send().await {
            return r.text().await.unwrap();
        }
        let _ = attempt;
    }
    String::new()
}

pub async fn poll(client: &reqwest::Client) {
    loop {
        let _ = client.get("https://api.example.com/status").send().await;
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

pub async fn worker(mut rx: tokio::sync::mpsc::Receiver<String>, client: reqwest::Client) {
    while let Some(job) = rx.recv().await {
        let _ = client.post("https://api.example.com/jobs").body(job).send().await;
    }
}

pub async fn purge(pool: &sqlx::PgPool) {
    for table in TABLES {
        sqlx::query("DELETE FROM users").bind(table).execute(pool).await.unwrap();
    }
}

pub async fn fetch_each(client: &reqwest::Client, ids: &[String]) {
    for id in ids {
        let _ = client.get("https://api.example.com/users").query(&[("id", id)]).send().await;
    }
}

pub fn walk(media: &[Media]) -> usize {
    let mut n = 0;
    for m in media {
        for t in &m.tracks {
            for p in t.parts.iter() {
                n += p.len();
            }
        }
    }
    n
}

pub fn pairs(items: &[u32]) -> usize {
    let mut n = 0;
    for a in items {
        for b in items {
            if a == b {
                n += 1;
            }
        }
    }
    n
}

pub fn scores(rows: &[u32]) -> u32 {
    let mut total = 0;
    for weight in [1, 2] {
        for r in rows {
            total += weight * r;
        }
    }
    total
}

pub fn inbox_files(root: std::path::PathBuf) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    let mut pending = vec![root];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            files.push(entry.path());
        }
    }
    files
}

pub fn cross(items: &[u32], other: &[u32]) -> usize {
    let mut n = 0;
    for a in items {
        let all = other.to_vec();
        for b in &all {
            n += (a + b) as usize;
        }
    }
    n
}

pub fn triples(items: &[u32]) -> usize {
    let mut n = 0;
    for a in items {
        for b in items {
            for c in items {
                n += (a + b + c) as usize;
            }
        }
    }
    n
}

pub fn per_group(groups: &[Vec<u32>]) -> usize {
    let mut n = 0;
    for g in groups {
        n += pairs(g);
    }
    n
}

pub fn against_all(groups: &[Vec<u32>], items: &[u32]) -> usize {
    let mut n = 0;
    for g in groups {
        n += pairs(items) + g.len();
    }
    n
}
