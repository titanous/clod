use std::process::Command;
#[test]
fn unsafe_startup_settings_are_rejected() {
    for (name, value, expected) in [
        ("CLOD_API_KEY", "", "must not be empty"),
        ("CLOD_ALIGN_TOOL_NAMES", "0", "must be 1"),
        ("CLOD_ALIGN_TOOL_NAMES", "true", "must be 1"),
        ("CLOD_ALIGN_TOOL_NAMES", "", "must be 1"),
        (
            "CLOD_UPSTREAM_BASE_URL",
            "https://example.com",
            "http loopback",
        ),
        (
            "CLOD_UPSTREAM_BASE_URL",
            "http://localhost.evil:42",
            "http loopback",
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_clod"))
            .env_remove("CLOD_SESSION_KEY")
            .env_remove("CLOD_ALIGN_TOOL_NAMES")
            .env_remove("CLOD_UPSTREAM_BASE_URL")
            .env_remove("CLOD_API_KEY")
            .env("CLOD_WORKDIR", dir.path())
            .env(name, value)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(expected),
            "{:?}",
            output
        );
    }
}
#[test]
fn workdir_below_git_repository_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join(".git")).unwrap();
    let work = dir.path().join("nested/empty");
    let output = Command::new(env!("CARGO_BIN_EXE_clod"))
        .env("CLOD_WORKDIR", work)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("inside a git repository"));
}
