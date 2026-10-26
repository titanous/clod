//! Fixture-driven process used only by shell integration tests.
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    env, fs,
    io::{self, BufRead, Write},
    path::PathBuf,
    sync::{mpsc, Arc, Mutex},
    thread,
    time::Duration,
};
#[derive(Deserialize)]
struct Scenario {
    fresh: Script,
    resume: Option<Script>,
}
#[derive(Deserialize)]
struct Script {
    fixture: Option<String>,
    #[serde(default)]
    delay_ms: u64,
    #[serde(default)]
    eof: bool,
    #[serde(default)]
    hang: bool,
    refusal: Option<String>,
    #[serde(default)]
    close_stdout: bool,
    #[serde(default)]
    no_stdin: bool,
    #[serde(default)]
    exit_delay_ms: u64,
}
struct Log {
    path: PathBuf,
    bytes: Vec<u8>,
}
fn record(file: &Mutex<Log>, value: Value) {
    let mut log = file.lock().unwrap();
    writeln!(log.bytes, "{value}").unwrap();
    // Shell tests inspect logs before the child exits. Publish a complete
    // snapshot so concurrent readers never observe a partly serialized record.
    let temporary = log.path.with_extension("tmp");
    fs::write(&temporary, &log.bytes).unwrap();
    fs::rename(temporary, &log.path).unwrap();
}
fn main() {
    let args: Vec<_> = env::args().skip(1).collect();
    if args == ["--version"] {
        println!("2.1.308 (Claude Code)");
        return;
    }
    let path = PathBuf::from(env::var_os("FAKE_CLAUDE_SCENARIO").expect("FAKE_CLAUDE_SCENARIO"));
    let scenario: Scenario = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let script = if args.iter().any(|a| a == "--resume") {
        scenario.resume.expect("unexpected resume")
    } else {
        scenario.fresh
    };
    let file = Arc::new(Mutex::new(Log {
        path: path
            .parent()
            .unwrap()
            .join(format!("{}.log", std::process::id())),
        bytes: Vec::new(),
    }));
    record(
        &file,
        json!({"argv":args,"env":env::vars().collect::<std::collections::BTreeMap<_,_>>(),"cwd":env::current_dir().unwrap(),"pid":std::process::id()}),
    );
    if script.no_stdin {
        loop {
            thread::park();
        }
    }
    let (tx, rx) = mpsc::channel();
    let log = file.clone();
    thread::spawn(move || {
        for line in io::stdin().lock().lines() {
            let line = line.unwrap();
            let value: Value = serde_json::from_str(&line).expect("stdin must be NDJSON");
            match value["type"].as_str() {
                Some("control_request") => assert!(
                    matches!(
                        value["request"]["subtype"].as_str(),
                        Some("initialize" | "interrupt")
                    ),
                    "unexpected control request: {value}"
                ),
                Some("user" | "control_response") => (),
                _ => panic!("unexpected stdin: {value}"),
            }
            record(&log, json!({"stdin":value}));
            let _ = tx.send(Some(value));
        }
        record(&log, json!({"eof":true}));
        let _ = tx.send(None);
    });
    let init = rx.recv().unwrap().expect("initialize");
    assert_eq!(init["request"]["subtype"], "initialize");
    assert_eq!(rx.recv().unwrap().expect("user")["type"], "user");
    thread::sleep(Duration::from_millis(script.delay_ms));
    if let Some(refusal) = script.refusal {
        if script.close_stdout {
            // SAFETY: close only the fake process's stdout to exercise cross-pipe ordering.
            unsafe {
                libc::close(libc::STDOUT_FILENO);
            }
            thread::sleep(Duration::from_millis(150));
        }
        eprintln!("{refusal}");
        std::process::exit(1);
    }
    if let Some(fixture) = script.fixture {
        let mut hooks = Vec::new();
        for line in fs::read_to_string(fixture).unwrap().lines() {
            let record: Value = serde_json::from_str(line).unwrap();
            if record["dir"] != "out" {
                continue;
            }
            let value = &record["line"];
            println!("{value}");
            io::stdout().flush().unwrap();
            if value["type"] == "control_request" {
                if value["request"]["subtype"] == "hook_callback" {
                    hooks.push(value["request_id"].clone());
                } else {
                    loop {
                        let input = rx.recv().unwrap().expect("control response");
                        if input["response"]["request_id"] == value["request_id"] {
                            break;
                        }
                    }
                }
            }
            if value["type"] == "stream_event"
                && matches!(
                    value["event"]["type"].as_str(),
                    Some("message_delta" | "message_stop")
                )
            {
                for id in hooks.drain(..) {
                    loop {
                        let input = rx.recv().unwrap().expect("hook reply");
                        if input["response"]["request_id"] == id {
                            break;
                        }
                    }
                }
            }
            if value["type"] == "result" {
                break;
            }
        }
    }
    if script.hang {
        loop {
            thread::park();
        }
    }
    if script.eof {
        while rx.recv().unwrap().is_some() {}
    }
    thread::sleep(Duration::from_millis(script.exit_delay_ms));
    record(&file, json!({"exiting":true}));
}

#[test]
fn published_logs_are_complete_snapshots() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("child.log");
    let log = Mutex::new(Log {
        path: path.clone(),
        bytes: Vec::new(),
    });
    record(&log, json!({"first":true}));
    let old_reader = fs::File::open(&path).unwrap();
    record(&log, json!({"second":"x".repeat(65536)}));
    let old: Vec<Value> = io::BufReader::new(old_reader)
        .lines()
        .map(|line| serde_json::from_str(&line.unwrap()).unwrap())
        .collect();
    let new: Vec<Value> = fs::read_to_string(&path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(old, vec![json!({"first":true})]);
    assert_eq!(new.len(), 2);
    assert_eq!(new[1]["second"].as_str().unwrap().len(), 65536);
    assert!(!path.with_extension("tmp").exists());
}
