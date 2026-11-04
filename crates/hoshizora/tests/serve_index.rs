use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

struct Server {
    child: Child,
    port: u16,
    data: PathBuf,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.data);
    }
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn scratch(name: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("hoshizora-{name}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn serve(project: &Path) -> Server {
    let data = scratch("data");
    let port = free_port();
    let child = Command::new(env!("CARGO_BIN_EXE_hoshizora"))
        .arg("serve")
        .arg("--data-dir")
        .arg(&data)
        .args(["--port", &port.to_string()])
        .args(["--otlp-port", &free_port().to_string()])
        .args(["--refresh-interval", "0"])
        .arg("--project")
        .arg(project)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    Server { child, port, data }
}

fn http(port: u16, method: &str, path: &str) -> Option<Value> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    )
    .ok()?;
    let mut response = String::new();
    stream.read_to_string(&mut response).ok()?;
    let (_, body) = response.split_once("\r\n\r\n")?;
    serde_json::from_str(body).ok()
}

fn project(server: &Server) -> Option<Value> {
    http(server.port, "GET", "/api/projects")?
        .as_array()?
        .first()
        .cloned()
}

fn wait_until(server: &Server, done: impl Fn(&Value) -> bool) -> Value {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if let Some(p) = project(server).filter(|p| done(p)) {
            return p;
        }
        assert!(
            Instant::now() < deadline,
            "timed out, last: {:?}",
            project(server)
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn settled(p: &Value) -> bool {
    matches!(p["status"].as_str(), Some("ready" | "failed"))
}

#[test]
fn serve_indexes_and_reindexes_through_the_child_indexer() {
    let src = scratch("src");
    std::fs::create_dir_all(src.join("src")).unwrap();
    std::fs::write(src.join("src/lib.rs"), "pub fn alpha() {}\n").unwrap();
    let server = serve(&src);

    let first = wait_until(&server, settled);
    assert_eq!(first["status"], "ready", "{first}");
    let id = first["id"].as_str().unwrap().to_string();
    let nodes = first["nodes"].as_u64().unwrap();
    assert!(nodes > 0);

    std::fs::write(
        src.join("src/lib.rs"),
        "pub fn alpha() {}\npub fn beta() {}\n",
    )
    .unwrap();
    http(server.port, "POST", &format!("/api/projects/{id}/reindex")).unwrap();
    let second = wait_until(&server, |p| {
        settled(p) && p["nodes"].as_u64() != Some(nodes)
    });
    assert_eq!(second["status"], "ready", "{second}");
    assert!(second["nodes"].as_u64().unwrap() > nodes);

    let graphs: Vec<_> = std::fs::read_dir(server.data.join("graphs"))
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(graphs, vec![format!("{id}.json")]);

    std::fs::remove_dir_all(&src).unwrap();
    http(server.port, "POST", &format!("/api/projects/{id}/reindex")).unwrap();
    let failed = wait_until(&server, |p| p["status"] == "failed");
    let error = failed["error"].as_str().unwrap();
    assert!(error.starts_with("indexer exit status"), "{error}");
    assert!(error.contains(&src.display().to_string()), "{error}");
}
