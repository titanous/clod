use futures_util::StreamExt;
use serde_json::{json, Value};
use std::{fs, path::PathBuf, process::Stdio, time::Duration};
use tempfile::TempDir;
use tokio::process::{Child, Command};

struct Server {
    _dir: TempDir,
    child: Child,
    url: String,
    scenario: PathBuf,
}
impl Drop for Server {
    fn drop(&mut self) {
        if std::thread::panicking() {
            // Never turn a test failure into a second panic during unwinding.
            let remaining: Vec<_> = fs::read_dir(self.scenario.parent().unwrap_or(&self.scenario))
                .into_iter()
                .flatten()
                .filter_map(Result::ok)
                .filter(|entry| entry.path().extension().is_some_and(|e| e == "log"))
                .filter_map(|entry| fs::read_to_string(entry.path()).ok())
                .filter_map(|text| serde_json::from_str::<Value>(text.lines().next()?).ok())
                .filter_map(|record| record["pid"].as_i64())
                .filter(|pid| unsafe { libc::kill(*pid as i32, 0) == 0 })
                .collect();
            eprintln!("test unwinding; skipping cleanup wait/assert; surviving fake CLI PIDs: {remaining:?}");
            let _ = self.child.start_kill();
            return;
        }
        // HTTP completion precedes runner cleanup. Keep the external server alive
        // until it has reaped every fake CLI, including timeout/hang scenarios.
        let deadline = std::time::Instant::now() + Duration::from_secs(8);
        let remaining = loop {
            let remaining: Vec<_> = self
                .logs()
                .iter()
                .filter_map(|log| log.first()?["pid"].as_i64())
                .filter(|pid| {
                    // SAFETY: signal zero checks existence without sending a signal.
                    unsafe { libc::kill(*pid as i32, 0) == 0 }
                })
                .collect();
            if remaining.is_empty() || std::time::Instant::now() >= deadline {
                break remaining;
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        let _ = self.child.start_kill();
        assert!(
            remaining.is_empty(),
            "server left fake CLI processes: {remaining:?}"
        );
    }
}
fn fixture(name: &str) -> String {
    format!(
        "{}/../clod-core/tests/fixtures/{name}.ndjson",
        env!("CARGO_MANIFEST_DIR")
    )
}
#[tokio::test]
async fn tool_name_alignment_environment_opt_in_reaches_fake_cli() {
    for enabled in [false, true] {
        let server = Server::start_aligned(text(), None, false, false, enabled).await;
        assert!(server
            .post(&request(false))
            .send()
            .await
            .unwrap()
            .status()
            .is_success());
        let logs = server.logs();
        let encoded = serde_json::to_string(&logs).unwrap();
        assert_eq!(encoded.contains("clod-tool-names"), enabled);
        if enabled {
            assert!(encoded.contains("mcp__oc__lookup"));
            assert!(encoded.contains("not the original execution names"));
        }
    }
}
fn text() -> Value {
    json!({"fixture":fixture("a-text"),"eof":true})
}
fn request(stream: bool) -> Value {
    json!({"model":"haiku","system":"test","max_tokens":2048,"stream":stream,"messages":[{"role":"user","content":"Remember the word cobalt. Reply only OK."}],"tools":[{"name":"lookup","description":"lookup","input_schema":{"type":"object","properties":{"key":{"type":"string"}}}}]})
}
impl Server {
    async fn start(fresh: Value, resume: Option<Value>, auth: bool, bad_bin: bool) -> Self {
        Self::start_aligned(fresh, resume, auth, bad_bin, false).await
    }
    async fn start_aligned(
        fresh: Value,
        resume: Option<Value>,
        auth: bool,
        bad_bin: bool,
        aligned: bool,
    ) -> Self {
        static BUILD: std::sync::Once = std::sync::Once::new();
        BUILD.call_once(|| {
            assert!(std::process::Command::new(env!("CARGO"))
                .args([
                    "build",
                    "-p",
                    "clod",
                    "--features",
                    "test-support",
                    "--bin",
                    "fake-claude"
                ])
                .status()
                .unwrap()
                .success());
        });
        let dir = tempfile::tempdir().unwrap();
        let scenario = dir.path().join("scenario.json");
        fs::write(
            &scenario,
            json!({"fresh":fresh,"resume":resume}).to_string(),
        )
        .unwrap();
        let bin = PathBuf::from(env!("CARGO_BIN_EXE_clod")).with_file_name("fake-claude");
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_clod"));
        cmd.env("CLOD_LISTEN", "127.0.0.1:0")
            .env("CLOD_WORKDIR", dir.path().join("workdir"))
            .env(
                "CLOD_CLAUDE_BIN",
                if bad_bin {
                    PathBuf::from("/nonexistent/clod-test")
                } else {
                    bin
                },
            )
            .env("FAKE_CLAUDE_SCENARIO", &scenario)
            .env("RUST_LOG", "info")
            .env("NO_COLOR", "1")
            .env("CLOD_TURN_TIMEOUT_MS", "3000")
            .env("CLOD_MAX_CONCURRENT", "2")
            .env("CLOD_MEASURE_LOG", dir.path().join("measure.ndjson"))
            .env("ANTHROPIC_API_KEY", "must-remove")
            .env("ANTHROPIC_CUSTOM_SETTING", "must-remove")
            .env("CLAUDE_CODE_USE_BEDROCK", "must-remove")
            .env("CLAUDE_CODE_USE_VERTEX", "must-remove")
            .env("CLOD_PRIVATE_TEST", "must-remove")
            .env("NO_PROXY", "localhost,127.0.0.1")
            .env("CLAUDE_CONFIG_DIR", "must-remove")
            .env_remove("CLOD_API_KEY")
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);
        cmd.env_remove("CLOD_ALIGN_TOOL_NAMES");
        if aligned {
            cmd.env("CLOD_ALIGN_TOOL_NAMES", "1");
        }
        if auth {
            cmd.env("CLOD_API_KEY", "secret");
        }
        let mut child = cmd.spawn().unwrap();
        use tokio::io::AsyncBufReadExt;
        let mut output = tokio::io::BufReader::new(child.stdout.take().unwrap()).lines();
        let address = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let line = output
                    .next_line()
                    .await
                    .unwrap()
                    .expect("server exited before listening");
                if let Some((_, address)) = line.split_once("address=") {
                    break address.trim().to_owned();
                }
            }
        })
        .await
        .unwrap();
        let server = Self {
            _dir: dir,
            child,
            url: format!("http://{address}"),
            scenario,
        };
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if reqwest::Client::new()
                    .get(format!("{}/health", server.url))
                    .header("x-api-key", "secret")
                    .send()
                    .await
                    .is_ok()
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        server
    }
    fn post(&self, body: &Value) -> reqwest::RequestBuilder {
        reqwest::Client::new()
            .post(format!("{}/v1/messages", self.url))
            .header("x-polytoken-conversation", "same")
            .json(body)
            .timeout(Duration::from_secs(15))
    }
    fn logs(&self) -> Vec<Vec<Value>> {
        let mut entries: Vec<_> = fs::read_dir(self.scenario.parent().unwrap())
            .unwrap()
            .map(Result::unwrap)
            .filter(|p| p.path().extension().is_some_and(|e| e == "log"))
            .collect();
        entries.sort_by_key(|e| e.file_name());
        entries
            .into_iter()
            .map(|p| {
                fs::read_to_string(p.path())
                    .unwrap()
                    .lines()
                    .map(|line| serde_json::from_str(line).unwrap())
                    .collect()
            })
            .collect()
    }
    async fn wait_logs(&self, ready: impl Fn(&[Vec<Value>]) -> bool) -> Vec<Vec<Value>> {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let logs = self.logs();
                if ready(&logs) {
                    return logs;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("expected fake CLI observation before deadline")
    }
    async fn success(&self, body: &Value) -> Value {
        let response = self.post(body).send().await.unwrap();
        let status = response.status();
        let value: Value = response.json().await.unwrap();
        assert_eq!(status, 200, "{value}");
        value
    }
    async fn seed(&self) -> Value {
        let mut body = request(false);
        let answer = self.success(&body).await;
        body["messages"].as_array_mut().unwrap().extend([
            json!({"role":"assistant","content":answer["content"]}),
            json!({"role":"user","content":"Again"}),
        ]);
        body
    }
}
#[tokio::test]
async fn waiting_same_key_does_not_take_global_permit() {
    let server = Server::start(
        json!({"fixture":fixture("a-text"),"eof":true,"exit_delay_ms":2000}),
        Some(text()),
        false,
        false,
    )
    .await;
    let seed = server.seed().await;
    let pending_request = server.post(&seed);
    let (entered, ready) = tokio::sync::oneshot::channel();
    let pending = tokio::spawn(async move {
        let mut future = Box::pin(pending_request.send());
        let _ = futures_util::poll!(&mut future);
        let _ = entered.send(());
        future.await
    });
    ready.await.unwrap();
    let response = tokio::time::timeout(
        Duration::from_millis(1200),
        reqwest::Client::new()
            .post(format!("{}/v1/messages", server.url))
            .json(&request(false))
            .header("x-polytoken-conversation", "other")
            .send(),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(pending.await.unwrap().unwrap().status(), 200);
}

#[tokio::test]
async fn dropping_server_after_timeout_waits_for_child_reaping() {
    let server = Server::start(json!({"no_stdin":true}), None, false, false).await;
    let response = server.post(&request(false)).send().await.unwrap();
    assert_eq!(response.status(), 500);
    assert!(response.text().await.unwrap().contains("timed out"));
    let pid = server.logs()[0][0]["pid"].as_i64().unwrap() as i32;
    drop(server);
    // SAFETY: signal zero checks existence without sending a signal.
    assert_eq!(
        unsafe { libc::kill(pid, 0) },
        -1,
        "fake CLI {pid} survived teardown"
    );
}

#[tokio::test]
async fn blocked_stdin_still_times_out() {
    let server = Server::start(json!({"no_stdin":true}), None, false, false).await;
    let mut body = request(false);
    body["messages"][0]["content"] = json!("x".repeat(3 * 1024 * 1024));
    let reply = tokio::time::timeout(Duration::from_secs(6), server.post(&body).send())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reply.status(), 500);
    assert!(reply.text().await.unwrap().contains("timed out"));
}

#[tokio::test]
async fn response_ends_before_delayed_child_exit() {
    let server = Server::start(
        json!({"fixture":fixture("a-text"),"eof":true,"exit_delay_ms":1500}),
        None,
        false,
        false,
    )
    .await;
    let start = std::time::Instant::now();
    server.success(&request(false)).await;
    assert!(start.elapsed() < Duration::from_millis(1200));
    let logs = server.logs();
    assert!(!logs[0].iter().any(|v| v["exiting"] == true));
}

#[tokio::test]
async fn shell_spawn_matches_spec() {
    let server = Server::start(text(), None, false, false).await;
    let body = request(false);
    server.success(&body).await;
    let logs = server.logs();
    let log = &logs[0][0];
    let request = clod_core::request::parse(&serde_json::to_vec(&body).unwrap(), &[]).unwrap();
    let config = clod_core::spawn::Config::new(
        server
            .scenario
            .parent()
            .unwrap()
            .join("workdir")
            .to_str()
            .unwrap(),
    );
    let spec = clod_core::spawn::spec(&request, &clod_core::session::Plan::Fresh, &config);
    assert_eq!(log["argv"], json!(spec.args));
    for fd in log["inherited_fds"].as_array().unwrap() {
        let fd = fd.as_i64().unwrap() as i32;
        // SAFETY: F_GETFD only reads flags for this process's descriptor.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
        assert!(
            flags >= 0 && flags & libc::FD_CLOEXEC == 0,
            "CLI inherited unexpected fd {fd}; not inherited from test runner"
        );
        eprintln!("CLI inherited upstream non-CLOEXEC descriptor {fd}");
    }
    assert_eq!(
        PathBuf::from(log["cwd"].as_str().unwrap())
            .canonicalize()
            .unwrap(),
        PathBuf::from(spec.working_directory)
            .canonicalize()
            .unwrap()
    );
    for (key, value) in spec.env_set {
        assert_eq!(log["env"][key], value);
    }
    for key in spec.env_remove {
        assert!(log["env"].get(key).is_none());
    }
    for key in [
        "CLOD_PRIVATE_TEST",
        "CLOD_API_KEY",
        "CLOD_WORKDIR",
        "ANTHROPIC_CUSTOM_SETTING",
        "CLAUDE_CODE_USE_BEDROCK",
        "CLAUDE_CODE_USE_VERTEX",
    ] {
        assert!(log["env"].get(key).is_none(), "{key}");
    }
    assert_eq!(log["env"]["NO_PROXY"], "localhost,127.0.0.1");
    assert_eq!(
        logs[0]
            .iter()
            .filter(|r| r["stdin"]["request"]["subtype"] == "interrupt")
            .count(),
        0
    );
}
#[tokio::test]
async fn shell_immediate_exit_buffered_result() {
    let server = Server::start(json!({"fixture":fixture("a-text")}), None, false, false).await;
    assert_eq!(
        server.success(&request(false)).await["stop_reason"],
        "end_turn"
    );
}
#[tokio::test]
async fn shell_stdin_eof_reaps() {
    let server = Server::start(text(), None, false, false).await;
    server.success(&request(false)).await;
    // A same-key follow-up cannot run until the first Done releases the lock.
    server.success(&request(false)).await;
    assert!(server.logs()[0].iter().any(|r| r["eof"] == true));
    assert!(server.logs()[0].iter().any(|r| r["exiting"] == true));
    let pid = server.logs()[0][0]["pid"].as_i64().unwrap() as i32;
    // SAFETY: signal zero probes existence without signalling the process.
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
}
#[tokio::test]
async fn shell_spawn_failure_releases_lock() {
    let server = Server::start(text(), None, false, true).await;
    for _ in 0..2 {
        let response = server.post(&request(false)).send().await.unwrap();
        assert_eq!(response.status(), 500);
        assert_eq!(response.json::<Value>().await.unwrap()["type"], "error");
    }
}
async fn retry(script: Value, concurrent: bool) {
    let server = Server::start(text(), Some(script), false, false).await;
    let body = server.seed().await;
    if concurrent {
        let first = server.post(&body).send();
        let second = server.post(&body).send();
        let (a, b) = tokio::join!(first, second);
        assert_eq!(a.unwrap().json::<Value>().await.unwrap()["type"], "message");
        assert_eq!(b.unwrap().json::<Value>().await.unwrap()["type"], "message");
    } else {
        server.success(&body).await;
    }
    let logs = server
        .wait_logs(|logs| {
            logs.len() == if concurrent { 4 } else { 3 }
                && logs.iter().all(|log| {
                    log.iter().any(|r| r["eof"] == true)
                        || log[0]["argv"]
                            .as_array()
                            .unwrap()
                            .contains(&json!("--resume"))
                })
        })
        .await;
    assert_eq!(logs.len(), if concurrent { 4 } else { 3 });
    assert_eq!(
        logs.iter()
            .filter(|log| log[0]["argv"]
                .as_array()
                .unwrap()
                .contains(&json!("--resume")))
            .count(),
        1
    );
    assert!(logs.iter().all(|log| log.iter().any(|r| r["eof"] == true)
        || log[0]["argv"]
            .as_array()
            .unwrap()
            .contains(&json!("--resume"))));
}
#[tokio::test]
async fn shell_stale_retry_single_lock() {
    retry(
        json!({"fixture":fixture("f-missing-session"),"eof":true,"delay_ms":100}),
        true,
    )
    .await;
}
#[tokio::test]
async fn shell_stderr_refusal_retries_fresh() {
    retry(
        json!({"refusal":"No conversation found with session ID: missing"}),
        false,
    )
    .await;
}
#[tokio::test]
async fn shell_late_stderr_refusal_retries_fresh() {
    retry(
        json!({"refusal":"No message found with message.uuid of: missing","close_stdout":true}),
        false,
    )
    .await;
}
#[tokio::test]
async fn shell_error_child_exits_on_eof() {
    let server = Server::start(
        json!({"fixture":fixture("f-missing-session"),"eof":true}),
        None,
        false,
        false,
    )
    .await;
    let response = server.post(&request(false)).send().await.unwrap();
    assert_eq!(response.status(), 500);
    let _ = response.text().await.unwrap();
    server
        .wait_logs(|logs| {
            logs.first()
                .is_some_and(|log| log.iter().any(|r| r["eof"] == true))
        })
        .await;
}
#[tokio::test]
async fn shell_auth_required() {
    let server = Server::start(text(), None, true, false).await;
    assert_eq!(
        server.post(&request(false)).send().await.unwrap().status(),
        401
    );
    for (header, key) in [("x-api-key", "secret"), ("authorization", "bEaReR secret")] {
        assert_eq!(
            server
                .post(&request(false))
                .header(header, key)
                .send()
                .await
                .unwrap()
                .status(),
            200
        );
    }
}
#[tokio::test]
async fn shell_same_key_serialized() {
    let mut script = text();
    script["delay_ms"] = json!(150);
    let server = Server::start(script, None, false, false).await;
    let start = std::time::Instant::now();
    let body = request(false);
    let (a, b) = tokio::join!(server.success(&body), server.success(&body));
    assert_eq!(a["type"], "message");
    assert_eq!(b["type"], "message");
    assert!(start.elapsed() >= Duration::from_millis(300));
}
fn sse(text: &str) -> Vec<Value> {
    text.split("\n\n")
        .filter(|s| !s.is_empty())
        .map(|event| {
            assert!(event.starts_with("event: "));
            serde_json::from_str(
                event
                    .lines()
                    .find_map(|line| line.strip_prefix("data: "))
                    .unwrap(),
            )
            .unwrap()
        })
        .collect()
}
#[tokio::test]
async fn shell_streaming_text_and_tool() {
    for (name, reason) in [("a-text", "end_turn"), ("b-tool", "tool_use")] {
        let server = Server::start(
            json!({"fixture":fixture(name),"eof":true}),
            None,
            false,
            false,
        )
        .await;
        let response = server.post(&request(true)).send().await.unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(response.headers()["content-type"], "text/event-stream");
        let events = sse(&response.text().await.unwrap());
        assert_eq!(events.last().unwrap()["type"], "message_stop");
        assert_eq!(
            events
                .iter()
                .filter(|e| e["type"] == "message_delta")
                .count(),
            1
        );
        assert!(events.iter().any(|e| e["delta"]["stop_reason"] == reason));
        if reason == "tool_use" {
            assert!(events
                .iter()
                .any(|e| e["content_block"]["name"] == "lookup"));
        }
    }
}
#[tokio::test]
async fn shell_nonstreaming() {
    let server = Server::start(text(), None, false, false).await;
    let value = server.success(&request(false)).await;
    assert_eq!(value["content"][0]["text"], "OK");
    assert_eq!(value["type"], "message");
}
#[tokio::test]
async fn shell_disconnect_kills() {
    // Emit content without a result and ignore stdin EOF, requiring group kill.
    let dir = tempfile::tempdir().unwrap();
    let partial = dir.path().join("partial.ndjson");
    let lines = fs::read_to_string(fixture("a-text"))
        .unwrap()
        .lines()
        .filter(|line| {
            let value: Value = serde_json::from_str(line).unwrap();
            value["line"]["type"] != "result"
        })
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(&partial, lines).unwrap();
    let server = Server::start(json!({"fixture":partial,"hang":true}), None, false, false).await;
    let response = server.post(&request(true)).send().await.unwrap();
    let mut stream = response.bytes_stream();
    assert!(stream.next().await.is_some());
    let pid = server.logs()[0][0]["pid"].as_i64().unwrap() as i32;
    drop(stream);
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            // SAFETY: signal zero only probes whether the process exists.
            if unsafe { libc::kill(pid, 0) } == -1 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    // The timeout above bounds the process-exit observation. A second wall-clock
    // check here can fail solely because the task is rescheduled at the deadline.
}
