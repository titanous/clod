//! Bounded HTTP boundary probes. The configured child executable never exists.
use serde_json::{json, Value};
use std::{process::Stdio, time::Duration};
use tokio::{io::AsyncBufReadExt, process::Command};

#[tokio::test]
async fn http_auth_body_and_spawn_boundaries() {
    let dir = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_clod"))
        .env("CLOD_LISTEN", "127.0.0.1:0")
        .env("CLOD_WORKDIR", dir.path().join("workdir"))
        .env("CLOD_CLAUDE_BIN", "/nonexistent/clod-boundary-test")
        .env("CLOD_API_KEY", "secret")
        .env("CLOD_MAX_BODY_BYTES", "512")
        .env("CLOD_MAX_CONCURRENT", "2")
        .env("RUST_LOG", "info")
        .env("NO_COLOR", "1")
        .env_remove("CLOD_SESSION_KEY")
        .env_remove("CLOD_UPSTREAM_BASE_URL")
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut lines = tokio::io::BufReader::new(child.stdout.take().unwrap()).lines();
    let address = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let line = lines.next_line().await.unwrap().expect("server exited");
            if let Some((_, a)) = line.split_once("address=") {
                break a.trim().to_owned();
            }
        }
    })
    .await
    .unwrap();
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let url = format!("http://{address}");
    for (name, value, status) in [
        ("x-api-key", "secret", 200),
        ("X-API-KEY", "secret", 200),
        ("authorization", "Bearer secret", 200),
        ("authorization", "bEaReR secret", 200),
        ("authorization", "Basic secret", 401),
        ("authorization", "Bearer wrong", 401),
        ("x-api-key", "wrong", 401),
    ] {
        let response = client
            .get(format!("{url}/health"))
            .header(name, value)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), status, "{name}: {value}");
    }
    let request = json!({"model":"-flag\u{0000}\n","system":"-system\u{0000}\n","messages":[{"role":"user","content":"hello"}],"max_tokens":u64::MAX});
    let body = request.to_string();
    // JSON trailing whitespace permits an exact body size without changing its meaning.
    for length in [511, 512, 513] {
        let padded = format!("{body}{}", " ".repeat(length - body.len()));
        let response = client
            .post(format!("{url}/v1/messages"))
            .header("x-api-key", "secret")
            .header("content-type", "application/json")
            .header("x-polytoken-conversation", "same")
            .body(padded)
            .send()
            .await
            .unwrap();
        assert_eq!(
            response.status().as_u16(),
            if length > 512 { 400 } else { 500 }
        );
        let error: Value = response.json().await.unwrap();
        assert_eq!(error["type"], "error");
        assert!(error["error"]["message"].is_string());
    }
    // Repeated mixed identity requests after failures must all complete: no leaked key lock
    // or semaphore permit. This does not infer successful-child serialization from failures.
    let mut tasks = tokio::task::JoinSet::new();
    for i in 0..24 {
        let client = client.clone();
        let url = url.clone();
        let body = body.clone();
        tasks.spawn(async move {
            let mut req = client
                .post(format!("{url}/v1/messages"))
                .header("x-api-key", "secret")
                .body(body);
            if i % 3 != 0 {
                req = req.header(
                    "x-polytoken-conversation",
                    if i % 3 == 1 { "same" } else { "different" },
                );
            }
            assert_eq!(req.send().await.unwrap().status().as_u16(), 500);
        });
    }
    while let Some(result) = tasks.join_next().await {
        result.unwrap();
    }
    child.kill().await.unwrap();
    child.wait().await.unwrap();
}
