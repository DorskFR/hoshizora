pub async fn run(pool: sqlx::PgPool) {
    sqlx::query("UPDATE jobs SET taken = true").execute(&pool).await.unwrap();
}

pub fn start(pool: &sqlx::PgPool, workers: usize) {
    for _ in 0..workers {
        tokio::spawn(run(pool.clone()));
    }
}

pub async fn reap(mut child: tokio::process::Child) {
    let _ = child.wait().await;
}

pub async fn reap_sync(mut child: std::process::Child) {
    let _ = child.wait();
}

pub struct Jobs;
pub struct Hosts;

impl Jobs {
    pub async fn count(&self, pool: &sqlx::PgPool) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM jobs").fetch_one(pool).await.unwrap()
    }
}

impl Hosts {
    pub async fn count(&self, pool: &sqlx::PgPool) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM hosts").fetch_one(pool).await.unwrap()
    }
}

pub async fn joined(jobs: &Jobs, hosts: &Hosts, pool: &sqlx::PgPool) -> (i64, i64) {
    tokio::join!(
        jobs.count(pool),
        hosts.count(pool),
    )
}

pub async fn sequential(jobs: &Jobs, hosts: &Hosts, pool: &sqlx::PgPool) -> (i64, i64) {
    let a = jobs.count(pool).await;
    let b = hosts.count(pool).await;
    (a, b)
}

impl Jobs {
    pub async fn mark(&self, pool: &sqlx::PgPool) {
        sqlx::query("UPDATE jobs SET seen = true").execute(pool).await.unwrap();
    }
}

impl Hosts {
    pub async fn touch(&self, pool: &sqlx::PgPool) {
        sqlx::query("UPDATE hosts SET seen = true").execute(pool).await.unwrap();
    }

    pub async fn pending_jobs(&self, pool: &sqlx::PgPool) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM jobs WHERE seen").fetch_one(pool).await.unwrap()
    }
}

pub async fn mark_then_count(jobs: &Jobs, hosts: &Hosts, pool: &sqlx::PgPool) -> i64 {
    jobs.mark(pool).await;
    hosts.pending_jobs(pool).await
}

pub async fn two_writes(jobs: &Jobs, hosts: &Hosts, pool: &sqlx::PgPool) {
    jobs.mark(pool).await;
    hosts.touch(pool).await;
}

pub fn spawn_logged<F>(name: &'static str, work: F)
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    let _ = name;
    tokio::spawn(work);
}

fn wait_for(child: std::process::Child) {
    std::thread::sleep(std::time::Duration::from_secs(1));
    drop(child);
}

pub async fn wait_inline(child: std::process::Child) {
    wait_for(child);
}

pub async fn reap_later(child: std::process::Child) {
    spawn_logged("reap", async move {
        wait_for(child);
    });
}
