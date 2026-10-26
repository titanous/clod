use std::{path::PathBuf, process::Command};
fn guard() -> Command {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut command = Command::new("python3");
    command.arg("-I").arg(root.join("scripts/check-core-deps"));
    command
}
#[test]
fn core_dependency_allowlist() {
    let output = guard().output().expect("run dependency guard");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
#[test]
fn dependency_guard_rejects_extra() {
    let output = guard()
        .arg("--metadata")
        .arg(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/dependency-metadata-extra.json"))
        .output()
        .expect("run negative dependency guard");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr)
        .contains("unexpected dependencies: ['unreviewed-io 1.0.0']"));
}
