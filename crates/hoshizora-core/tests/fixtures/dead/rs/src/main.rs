use std::sync::LazyLock;

use axum::Router;
use axum::routing::post;
use clap::Parser;

#[derive(Parser)]
enum Cmd {
    Make {
        #[arg(long, value_parser = parse_expiry)]
        expiry: u64,
    },
}

fn parse_expiry(s: &str) -> Result<u64, String> {
    s.parse().map_err(|_| "bad".to_string())
}

static PATTERN: LazyLock<String> = LazyLock::new(|| build_pattern());

fn build_pattern() -> String {
    String::new()
}

fn find() -> u32 {
    0
}

mod mp4 {
    pub fn find() -> u32 {
        read_box()
    }

    fn read_box() -> u32 {
        1
    }
}

async fn create() {}

fn router() -> Router {
    Router::new().route("/x", post(create))
}

#[pyo3::pyfunction]
fn exported() -> u32 {
    ffi_helper()
}

fn ffi_helper() -> u32 {
    2
}

fn really_dead() {}

fn main() {
    let _ = mp4::find();
    let _ = router();
    let _ = Cmd::parse();
}
