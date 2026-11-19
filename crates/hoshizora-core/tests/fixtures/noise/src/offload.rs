pub struct App;

pub async fn blocking<T, F>(app: &App, work: F) -> T
where
    F: FnOnce(&App) -> T + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(move || work(app)).await.unwrap()
}

fn read_config(_app: &App) -> String {
    std::thread::sleep(std::time::Duration::from_millis(5));
    String::new()
}

pub async fn offloaded(app: &App) -> String {
    let config = blocking(app, move |app| {
        let text = read_config(app);
        text
    })
    .await;
    config
}

pub async fn inline(app: &App) -> String {
    read_config(app)
}

pub async fn same_line(app: &App) -> String {
    blocking(app, move |app| read_config(app)).await
}

pub async fn hash_off_thread(path: std::path::PathBuf) -> usize {
    tokio::task::spawn_blocking(move || {
        let bytes = std::fs::read(&path).unwrap();
        bytes.len()
    })
    .await
    .unwrap()
}

pub async fn hash_inline(path: std::path::PathBuf) -> usize {
    let bytes = std::fs::read(&path).unwrap();
    bytes.len()
}

pub fn start_ticker() {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        }
    });
}

pub fn nap() {
    std::thread::sleep(std::time::Duration::from_millis(5));
}

pub async fn boot_ticker() {
    start_ticker();
}

pub async fn boot_nap() {
    nap();
}
