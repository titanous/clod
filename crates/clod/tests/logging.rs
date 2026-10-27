#![cfg(feature = "test-support")]
//! Operational logging: info by default on stderr, a single address line on
//! stdout, correlated lifecycle records with final numeric usage, and no
//! prompt contents or raw session-key bytes.
use serde_json::{json, Value};
use std::{path::PathBuf, process::Stdio, time::Duration};
use tokio::io::{AsyncBufReadExt, BufReader};

#[tokio::test]
async fn default_logging_is_info_on_stderr_with_final_usage() {
    let dir = tempfile::tempdir().unwrap();
    let scenario = dir.path().join("scenario.json");
    let fixture = format!(
        "{}/../clod-core/tests/fixtures/a-text.ndjson",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::write(
        &scenario,
        json!({"fresh":{"fixture":fixture,"eof":true},"resume":null}).to_string(),
    )
    .unwrap();
    let measure = dir.path().join("measure.ndjson");
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_clod"))
        .env("CLOD_LISTEN", "127.0.0.1:0")
        .env("CLOD_WORKDIR", dir.path().join("workdir"))
        .env(
            "CLOD_CLAUDE_BIN",
            PathBuf::from(env!("CARGO_BIN_EXE_clod")).with_file_name("fake-claude"),
        )
        .env("FAKE_CLAUDE_SCENARIO", &scenario)
        .env("CLOD_MEASURE_LOG", &measure)
        .env("NO_COLOR", "1")
        .env_remove("RUST_LOG")
        .env_remove("CLOD_API_KEY")
        .env_remove("CLOD_UPSTREAM_BASE_URL")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut stderr = BufReader::new(child.stderr.take().unwrap()).lines();
    let address = tokio::time::timeout(Duration::from_secs(10), stdout.next_line())
        .await
        .unwrap()
        .unwrap()
        .expect("address line on stdout");
    let address = address
        .strip_prefix("listening address=")
        .unwrap_or_else(|| panic!("unexpected stdout line: {address}"))
        .to_owned();
    let response: Value = reqwest::Client::new()
        .post(format!("http://{address}/v1/messages"))
        .json(
            &json!({"model":"haiku","system":"test","max_tokens":64,"stream":false,
            "messages":[{"role":"user","content":"Remember the word cobalt. Reply only OK."}]}),
        )
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(response["content"][0]["text"], "OK");
    let mut logs = Vec::new();
    let finished = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(line) = stderr.next_line().await.unwrap() {
            let done = line.contains("request finished");
            logs.push(line);
            if done {
                return;
            }
        }
        panic!("clod exited before finishing the request");
    })
    .await;
    child.start_kill().unwrap();
    let logs = logs.join("\n");
    finished.unwrap_or_else(|_| panic!("no request finished log:\n{logs}"));
    for expected in [
        " INFO ",
        "listening",
        "request received",
        "session planned",
        "spawning CLI",
        "CLI exited",
        "request{id=",
        "attempt=1",
        "plan=\"fresh\"",
        "status=200",
        "input_tokens=2",
        "output_tokens=4",
        "cache_read_input_tokens=0",
        "cache_creation_input_tokens=1067",
    ] {
        assert!(logs.contains(expected), "missing {expected:?} in:\n{logs}");
    }
    assert!(!logs.contains("cobalt"), "prompt text logged:\n{logs}");
    assert!(!logs.contains("PrefixMismatch") && !logs.contains("Commit("));
    // Nothing else is written to stdout.
    assert!(stdout.next_line().await.unwrap().is_none());
    // Measurement records stay complete JSON lines with the same kinds.
    let records: Vec<Value> = std::fs::read_to_string(&measure)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let kinds: Vec<_> = records
        .iter()
        .map(|r| r["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds.first(), Some(&"decision"));
    assert!(kinds.contains(&"spawn") && kinds.contains(&"usage"));
    assert_eq!(kinds.last(), Some(&"effect"));
}
