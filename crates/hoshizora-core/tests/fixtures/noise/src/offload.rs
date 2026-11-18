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
