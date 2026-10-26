//! Explicitly authorized subscription checks; never run in routine offline checks.
use std::process::Command;

fn run(mode: &str) {
    assert_eq!(std::env::var("CLOD_LIVE").as_deref(), Ok("1"));
    assert_eq!(
        std::env::var("OPERATOR_APPROVED_LIVE_VERIFY").as_deref(),
        Ok("1"),
        "explicit operator approval marker is required"
    );
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let status = Command::new("python3")
        .args(["-I", "scripts/live-verify.py"])
        .current_dir(root)
        .env_remove("LIVE_IMAGE")
        .env_remove("LIVE_THINKING")
        .env_remove("LIVE_MIXED")
        .env_remove("LIVE_PROBE")
        .env_remove("LIVE_FORK_PROBE")
        .env_remove("LIVE_WRAPPER")
        .env(mode, "1")
        .env("LIVE_LIMIT", "4")
        .env("LIVE_CLOD_BIN", env!("CARGO_BIN_EXE_clod"))
        .status()
        .expect("start direct HTTP live recorder");
    assert!(status.success(), "live recorder failed: {status}");
}

#[test]
#[ignore = "three paid Haiku requests; requires CLOD_LIVE=1 and operator approval"]
fn live_image_resume() {
    run("LIVE_IMAGE");
}

#[test]
#[ignore = "three paid Sonnet requests; requires CLOD_LIVE=1 and operator approval"]
fn live_thinking_resume() {
    run("LIVE_THINKING");
}
