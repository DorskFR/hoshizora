pub struct Scheduler;

impl Scheduler {
    pub async fn start(&self) -> usize {
        let body = reqwest::get("http://ticks.example.com/start").await.unwrap();
        body.text().await.unwrap().len()
    }
}

pub struct Dispatcher;

impl Dispatcher {
    pub async fn dispatch(&self, key: &str) -> String {
        reqwest::get(format!("http://ticks.example.com/{key}")).await.unwrap().text().await.unwrap()
    }
}
