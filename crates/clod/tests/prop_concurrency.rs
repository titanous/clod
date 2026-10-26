//! Offline seeded stateful workloads. Reproduce with CLOD_CONCURRENCY_SEED=<seed>.
//! CLOD_CONCURRENCY_CASES increases the bounded default of four cases.
#![cfg(feature = "test-support")]
use futures_util::{future::join_all, StreamExt};
use serde_json::{json, Value};
use std::{fs, path::PathBuf, process::Stdio, time::Duration};
use tokio::{
    io::AsyncBufReadExt,
    process::{Child, Command},
};

struct Server {
    dir: tempfile::TempDir,
    child: Child,
    url: String,
}
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}
fn fixture(name: &str) -> String {
    format!(
        "{}/../clod-core/tests/fixtures/{name}.ndjson",
        env!("CARGO_MANIFEST_DIR")
    )
}
fn body(stream: bool) -> Value {
    json!({"model":"haiku","system":"test","max_tokens":2048,"stream":stream,"messages":[{"role":"user","content":"Remember the word cobalt. Reply only OK."}],"tools":[{"name":"lookup","description":"lookup","input_schema":{"type":"object","properties":{"key":{"type":"string"}}}}]})
}
fn alive(pid: i32) -> bool {
    // SAFETY: signal zero does not signal the process.
    unsafe { libc::kill(pid, 0) == 0 }
}
impl Server {
    async fn start(fresh: Value, resume: Value, cap: usize, bad: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let scenario = dir.path().join("scenario.json");
        fs::write(
            &scenario,
            json!({"fresh":fresh,"resume":resume}).to_string(),
        )
        .unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_clod"));
        command
            .env("CLOD_LISTEN", "127.0.0.1:0")
            .env("CLOD_WORKDIR", dir.path().join("work"))
            .env(
                "CLOD_CLAUDE_BIN",
                if bad {
                    PathBuf::from("/nonexistent/clod-concurrency")
                } else {
                    PathBuf::from(env!("CARGO_BIN_EXE_fake-claude"))
                },
            )
            .env("FAKE_CLAUDE_SCENARIO", scenario)
            .env("CLOD_MAX_CONCURRENT", cap.to_string())
            .env("CLOD_SESSION_CAP", "2")
            .env("CLOD_TURN_TIMEOUT_MS", "4000")
            .env("NO_PROXY", "127.0.0.1,localhost")
            .env_remove("CLOD_API_KEY")
            .env("RUST_LOG", "info")
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);
        let mut child = command.spawn().unwrap();
        let mut lines = tokio::io::BufReader::new(child.stdout.take().unwrap()).lines();
        let address = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let line = lines.next_line().await.unwrap().expect("server startup");
                if let Some((_, address)) = line.split_once("address=") {
                    break address.trim().to_owned();
                }
            }
        })
        .await
        .unwrap();
        Self {
            dir,
            child,
            url: format!("http://{address}"),
        }
    }
    fn post(&self, key: Option<usize>, value: &Value) -> reqwest::RequestBuilder {
        let request = reqwest::Client::new()
            .post(format!("{}/v1/messages", self.url))
            .json(value)
            .timeout(Duration::from_secs(10));
        if let Some(key) = key {
            request.header("x-polytoken-conversation", format!("key-{key}"))
        } else {
            request
        }
    }
    fn logs(&self) -> Vec<Vec<Value>> {
        fs::read_dir(self.dir.path())
            .unwrap()
            .map(Result::unwrap)
            .filter(|e| e.path().extension().is_some_and(|x| x == "log"))
            .map(|e| {
                fs::read_to_string(e.path())
                    .unwrap()
                    .lines()
                    .map(|l| serde_json::from_str(l).unwrap())
                    .collect()
            })
            .collect()
    }
    async fn reaped(&self) {
        tokio::time::timeout(Duration::from_secs(7), async {
            loop {
                if self
                    .logs()
                    .iter()
                    .all(|l| !alive(l[0]["pid"].as_i64().unwrap() as i32))
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("child was not reaped within cleanup budget");
    }
    async fn success(&self, key: Option<usize>, value: &Value) -> Value {
        let response = self.post(key, value).send().await.unwrap();
        assert_eq!(response.status(), 200);
        if value["stream"] == true {
            let text = response.text().await.unwrap();
            assert_eq!(text.matches("event: message_stop\n").count(), 1, "{text}");
            assert!(!text.contains("event: error\n"), "{text}");
            Value::Null
        } else {
            let result: Value = response.json().await.unwrap();
            assert_eq!(result["type"], "message");
            result
        }
    }
}
fn next(seed: &mut u64) -> u64 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 7;
    *seed ^= *seed << 17;
    *seed
}
fn resume(log: &[Value]) -> bool {
    log[0]["argv"]
        .as_array()
        .unwrap()
        .contains(&json!("--resume"))
}
fn assert_resume_at(log: &[Value], name: &str) {
    let records: Vec<Value> = fs::read_to_string(fixture(name))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let assistant = records
        .iter()
        .rev()
        .find(|r| r["dir"] == "out" && r["line"]["type"] == "assistant")
        .unwrap();
    let args = log[0]["argv"].as_array().unwrap();
    assert!(args.contains(&json!("--fork-session")));
    assert!(args
        .windows(2)
        .any(|w| w[0] == "--resume-session-at" && w[1] == assistant["line"]["uuid"]));
    assert!(args
        .windows(2)
        .any(|w| w[0] == "--resume" && w[1] == assistant["line"]["session_id"]));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn seeded_successful_and_mixed_workloads() {
    let initial = std::env::var("CLOD_CONCURRENCY_SEED")
        .ok()
        .map(|s| s.parse().unwrap())
        .unwrap_or(0x4c4f44434f4e);
    let cases = std::env::var("CLOD_CONCURRENCY_CASES")
        .ok()
        .map(|s| s.parse().unwrap())
        .unwrap_or(4);
    let mut rng = initial;
    let mut requests = 0;
    let mut resumes = 0;
    let mut retries = 0;
    let mut contended = 0;
    for case in 0..cases {
        let seed = rng;
        let cap = 1 + case % 2;
        eprintln!("concurrency reproduction seed={seed} initial={initial} case={case} cap={cap}");
        let tool = case % 2 == 1;
        let name = if tool { "b-tool" } else { "a-text" };
        let stale = case % 3 == 0;
        let delay = 20 + next(&mut rng) % 60;
        let server = Server::start(
            json!({"fixture":fixture(name),"eof":case%2==0,"delay_ms":delay,"exit_delay_ms":40}),
            if stale {
                json!({"fixture":fixture("f-missing-session"),"eof":true})
            } else {
                json!({"fixture":fixture("a-text-resume"),"eof":true,"delay_ms":delay})
            },
            cap,
            false,
        )
        .await;
        let mut follow = body(false);
        let answer = server.success(Some(0), &follow).await;
        requests += 1;
        let content = if tool {
            Value::Array(answer["content"].as_array().unwrap().iter().filter(|v| v["type"] == "tool_use").map(|v|json!({"type":"tool_result","tool_use_id":v["id"],"content":"VALUE_alpha"})).collect())
        } else {
            json!("Again")
        };
        follow["messages"].as_array_mut().unwrap().extend([
            json!({"role":"assistant","content":answer["content"]}),
            json!({"role":"user","content":content}),
        ]);
        // Queue immediately after body completion: cleanup still holds the key lock.
        let (a, b) = tokio::join!(
            server.success(Some(0), &follow),
            server.success(Some(0), &follow)
        );
        assert_eq!(a["type"], "message");
        assert_eq!(b["type"], "message");
        requests += 2;
        contended += 1;
        server.reaped().await;
        let mut logs = server.logs();
        logs.sort_by_key(|l| {
            l[0]["recorded_ns"]
                .as_str()
                .unwrap()
                .parse::<u128>()
                .unwrap()
        });
        assert!(
            resume(&logs[1]),
            "first queued follow-up missed committed checkpoint seed={seed}"
        );
        let resumed: Vec<_> = logs.iter().filter(|l| resume(l)).collect();
        assert!(
            !resumed.is_empty(),
            "follow-up missed checkpoint seed={seed}"
        );
        for log in resumed {
            assert_resume_at(log, name);
            resumes += 1;
        }
        if stale {
            assert_eq!(logs.len(), 4);
            retries += 1;
        }
        let workload: Vec<_> = (0..8)
            .map(|_| {
                (
                    match next(&mut rng) % 4 {
                        0 => None,
                        1 => Some(0),
                        _ => Some(1),
                    },
                    next(&mut rng) % 2 == 0,
                )
            })
            .collect();
        eprintln!("burst={workload:?}");
        join_all(workload.iter().map(|(key, stream)| {
            let mut value = body(*stream);
            value["messages"][0]["content"] = json!(format!("burst-key-{key:?}"));
            let server = &server;
            async move {
                server.success(*key, &value).await;
            }
        }))
        .await;
        requests += workload.len();
        contended += 1;
        for key in [Some(0), Some(1), None] {
            let mut probe = body(false);
            probe["messages"][0]["content"] = json!(format!("burst-key-{key:?}"));
            server.success(key, &probe).await;
            requests += 1;
        }
        server.reaped().await;
        // Script exit markers bracket observable lifetimes, not HTTP body completion.
        let logs = server.logs();
        for key in 0..2 {
            let marker = format!("burst-key-Some({key})");
            let mut spans: Vec<_> = logs
                .iter()
                .filter(|l| {
                    l.iter().any(|r| {
                        r.get("stdin")
                            .is_some_and(|v| v.to_string().contains(&marker))
                    })
                })
                .map(|l| {
                    (
                        l[0]["recorded_ns"]
                            .as_str()
                            .unwrap()
                            .parse::<u128>()
                            .unwrap(),
                        l.iter().find(|r| r["exiting"] == true).unwrap()["recorded_ns"]
                            .as_str()
                            .unwrap()
                            .parse::<u128>()
                            .unwrap(),
                    )
                })
                .collect();
            spans.sort();
            assert!(
                spans.windows(2).all(|w| w[0].1 <= w[1].0),
                "same-key child overlap seed={seed} key={key}: {spans:?}"
            );
        }
        let mut intervals: Vec<_> = logs
            .iter()
            .map(|l| {
                let start = l[0]["recorded_ns"]
                    .as_str()
                    .unwrap()
                    .parse::<u128>()
                    .unwrap();
                let end = l
                    .iter()
                    .find(|r| r["exiting"] == true)
                    .map(|r| r["recorded_ns"].as_str().unwrap().parse::<u128>().unwrap());
                // Stale fixture exits normally; refusal scripts are not used here.
                (start, end.expect("successful child exit marker"))
            })
            .flat_map(|(s, e)| [(s, 1_i32), (e, -1)])
            .collect();
        intervals.sort();
        let mut live = 0;
        for (_, delta) in intervals {
            live += delta;
            assert!(live <= cap as i32, "child cap exceeded seed={seed}");
        }
        assert_eq!(live, 0);
    }
    eprintln!("coverage requests={requests} resumes={resumes} stale_retries={retries} contended_phases={contended}");
}

#[tokio::test]
async fn spawn_failure_does_not_leak_permits_or_key_locks() {
    let server = Server::start(json!({}), json!({}), 1, true).await;
    for _ in 0..3 {
        for key in [Some(0), Some(1), None] {
            let response = tokio::time::timeout(
                Duration::from_secs(2),
                server.post(key, &body(false)).send(),
            )
            .await
            .expect("spawn failure leaked permit or lock")
            .unwrap();
            assert_eq!(response.status(), 500);
            assert_eq!(response.json::<Value>().await.unwrap()["type"], "error");
        }
    }
}

#[tokio::test]
async fn disconnect_after_first_chunk_reaps_child() {
    let server = Server::start(
        json!({"fixture":fixture("a-text"),"hang":true,"delay_ms":30}),
        json!({}),
        1,
        false,
    )
    .await;
    let response = server.post(Some(0), &body(true)).send().await.unwrap();
    let mut stream = response.bytes_stream();
    assert!(stream.next().await.is_some());
    drop(stream);
    // Successful result may already be buffered: cleanup must still kill a child ignoring EOF.
    server.reaped().await;
    assert!(!server.logs().is_empty());
}
