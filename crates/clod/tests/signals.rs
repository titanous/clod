#![cfg(feature = "test-support")]

use serde_json::{json, Value};
use std::{
    fs,
    io::{BufRead, BufReader},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

fn exists(pid: i32) -> bool {
    // SAFETY: signal zero only probes existence.
    unsafe { libc::kill(pid, 0) == 0 }
}
fn signal(pid: i32, binary: &str, signal: i32) -> bool {
    let output = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "args="])
        .output()
        .unwrap();
    let args = String::from_utf8(output.stdout).unwrap();
    if args.split_whitespace().next() != Some(binary) {
        // It may have exited between try_wait()/exists() and this identity check.
        // Never signal a zombie or an unrelated process that reused the PID.
        return false;
    }
    // SAFETY: this is our spawned process, whose executable was just verified.
    unsafe { libc::kill(pid, signal) == 0 }
}
struct Server {
    child: Child,
    dir: tempfile::TempDir,
}
impl Server {
    fn pids(&self) -> Vec<i32> {
        let mut pids = vec![];
        for entry in fs::read_dir(self.dir.path()).unwrap().map(Result::unwrap) {
            if entry.path().extension().is_some_and(|e| e == "log") {
                for line in fs::read_to_string(entry.path()).unwrap().lines() {
                    let record: Value = serde_json::from_str(line).unwrap();
                    for key in ["pid", "grandchild_pid"] {
                        if let Some(pid) = record[key].as_i64() {
                            pids.push(pid as i32);
                        }
                    }
                }
            }
        }
        pids
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        // Failure cleanup only targets recorded processes with verified identities.
        for pid in self.pids() {
            if exists(pid) {
                signal(pid, env!("CARGO_BIN_EXE_fake-claude"), libc::SIGKILL);
            }
        }
        if self.child.try_wait().unwrap().is_none() {
            signal(
                self.child.id() as i32,
                env!("CARGO_BIN_EXE_clod"),
                libc::SIGKILL,
            );
        }
        self.child.wait().unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while self.pids().iter().any(|&pid| exists(pid)) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
fn shutdown(signal_number: i32, live: bool, repeated: bool) {
    shutdown_with_stall(signal_number, live, repeated, false);
}
fn shutdown_with_stall(signal_number: i32, live: bool, repeated: bool, stalled: bool) {
    let dir = tempfile::tempdir().unwrap();
    let scenario = dir.path().join("scenario.json");
    fs::write(
        &scenario,
        json!({"fresh":{"no_stdin":true,"grandchild":true}}).to_string(),
    )
    .unwrap();
    let marker = dir.path().join("blocked");
    let mut command = Command::new(env!("CARGO_BIN_EXE_clod"));
    if stalled {
        command.env("CLOD_TEST_BLOCK_WORKER", &marker);
    } else {
        command.env_remove("CLOD_TEST_BLOCK_WORKER");
    }
    let child = command
        .env("CLOD_LISTEN", "127.0.0.1:0")
        .env("CLOD_WORKDIR", dir.path().join("workdir"))
        .env("CLOD_CLAUDE_BIN", env!("CARGO_BIN_EXE_fake-claude"))
        .env("FAKE_CLAUDE_SCENARIO", scenario)
        .env("RUST_LOG", "info")
        .env("NO_COLOR", "1")
        .env_remove("CLOD_API_KEY")
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut server = Server { child, dir };
    let output = server.child.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(output).lines() {
            if let Some((_, address)) = line.unwrap().split_once("address=") {
                let _ = tx.send(address.trim().to_owned());
                break;
            }
        }
    });
    let address = rx.recv_timeout(Duration::from_secs(10)).unwrap();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    if live {
        runtime.spawn(async move {
            let _ = reqwest::Client::new()
                .post(format!("http://{address}/v1/messages"))
                .json(&json!({"model":"haiku","max_tokens":16,"messages":[{"role":"user","content":"test"}]}))
                .send().await;
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        while server.pids().len() != 2 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            server.pids().len(),
            2,
            "leader and grandchild must publish PIDs"
        );
    }
    if stalled {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !marker.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(marker.exists(), "runner must be synchronously blocked");
    }
    let started = Instant::now();
    assert!(signal(
        server.child.id() as i32,
        env!("CARGO_BIN_EXE_clod"),
        signal_number,
    ));
    if repeated && server.child.try_wait().unwrap().is_none() {
        // Keep a second registered signal pending without introducing a timing sleep.
        signal(
            server.child.id() as i32,
            env!("CARGO_BIN_EXE_clod"),
            signal_number,
        );
    }
    let deadline = Instant::now() + Duration::from_secs(if stalled { 6 } else { 3 });
    let status = loop {
        if let Some(status) = server.child.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "clod did not exit promptly");
        std::thread::sleep(Duration::from_millis(10));
    };
    while server.pids().iter().any(|&pid| exists(pid)) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        server.pids().iter().all(|&pid| !exists(pid)),
        "CLI process group survived signal shutdown: {:?}",
        server.pids()
    );
    assert_eq!(status.code(), Some(128 + signal_number));
    if stalled {
        assert!(
            started.elapsed() >= Duration::from_millis(2800),
            "must exercise shutdown timeout, not ordinary worker cancellation"
        );
    }
}
#[test]
fn sigterm_kills_cli_group() {
    shutdown(libc::SIGTERM, true, false);
}
#[test]
fn sighup_kills_cli_group() {
    shutdown(libc::SIGHUP, true, false);
}
#[test]
fn blocked_worker_shutdown_is_bounded_and_kills_cli_group() {
    shutdown_with_stall(libc::SIGTERM, true, true, true);
}
#[test]
fn sigint_kills_cli_group() {
    shutdown(libc::SIGINT, true, false);
}
#[test]
fn signals_without_requests_exit_promptly() {
    shutdown(libc::SIGTERM, false, false);
    shutdown(libc::SIGINT, false, false);
}
#[test]
fn repeated_signal_exits_promptly() {
    shutdown(libc::SIGTERM, true, true);
}
