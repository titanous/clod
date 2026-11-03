#![cfg(feature = "test-support")]

use serde_json::{json, Value};
use std::{
    fs,
    time::{Duration, Instant},
};

#[test]
fn runtime_shutdown_kills_cli_process_group() {
    let dir = tempfile::tempdir().unwrap();
    let scenario = dir.path().join("scenario.json");
    fs::write(
        &scenario,
        json!({"fresh":{"no_stdin":true,"grandchild":true}}).to_string(),
    )
    .unwrap();
    // This integration-test binary has one test; set the environment before Tokio starts.
    std::env::set_var("FAKE_CLAUDE_SCENARIO", &scenario);
    let binary = env!("CARGO_BIN_EXE_fake-claude");
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let (leader, descendant) = runtime.block_on(async {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/v1/messages", listener.local_addr().unwrap());
        let mut core = clod_core::spawn::Config::new(dir.path().to_str().unwrap());
        core.program = binary.into();
        let app = clod::router(clod::Settings {
            listen: String::new(),
            api_key: None,
            core,
            max_concurrent: 1,
            session_cap: 1,
            max_body_bytes: 1024 * 1024,
            allow_any_host: false,
            weekly_breaker: false,
        });
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        tokio::spawn(async move {
            let _response = reqwest::Client::new()
                .post(url)
                .json(&json!({
                    "model":"haiku", "max_tokens":16,
                    "messages":[{"role":"user","content":"test"}]
                }))
                .send()
                .await;
            std::future::pending::<()>().await;
        });
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                for entry in fs::read_dir(dir.path()).unwrap().map(Result::unwrap) {
                    if entry.path().extension().is_some_and(|e| e == "log") {
                        let records: Vec<Value> = fs::read_to_string(entry.path())
                            .unwrap()
                            .lines()
                            .map(|l| serde_json::from_str(l).unwrap())
                            .collect();
                        if let Some(pid) = records.iter().find_map(|r| r["grandchild_pid"].as_i64())
                        {
                            return (records[0]["pid"].as_i64().unwrap() as i32, pid as i32);
                        }
                    }
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("fake CLI must publish descendant PID")
    });
    // Both processes are in the group created by Process::spawn, not our group.
    assert_eq!(unsafe { libc::getpgid(descendant) }, leader);
    assert_ne!(leader, unsafe { libc::getpgrp() });
    // Runtime destruction cancels the actual detached runner without on_tick/on_exit.
    drop(runtime);
    let deadline = Instant::now() + Duration::from_secs(3);
    while unsafe { libc::kill(descendant, 0) } == 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    let survived = unsafe { libc::kill(descendant, 0) } == 0;
    if survived {
        // Clean only this test's recorded descendant after verifying executable identity.
        let identity_matches = std::process::Command::new("ps")
            .args(["-p", &descendant.to_string(), "-o", "args="])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .is_some_and(|args| args.split_whitespace().next() == Some(binary));
        if identity_matches && unsafe { libc::getpgid(descendant) } == leader {
            unsafe {
                libc::kill(descendant, libc::SIGKILL);
            }
        } else {
            eprintln!("refusing cleanup of PID {descendant}: executable/group identity changed");
        }
    }
    assert!(
        !survived,
        "descendant {descendant} survived runtime shutdown (leader {leader})"
    );
}
