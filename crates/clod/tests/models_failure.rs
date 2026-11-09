#![cfg(feature = "test-support")]
use clod::{router, Settings};
use clod_core::spawn::Config;

#[tokio::test]
async fn concurrent_failures_share_one_probe() {
    let dir = tempfile::tempdir().unwrap();
    let scenario = dir.path().join("scenario.json");
    std::fs::write(&scenario, r#"{"fresh":{"refusal":"fail discovery"}}"#).unwrap();
    let mut core = Config::new(dir.path().to_string_lossy());
    core.program = env!("CARGO_BIN_EXE_fake-claude").into();
    core.upstream_base_url = Some(scenario.to_string_lossy().into_owned());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1/models", listener.local_addr().unwrap());
    let app = router(Settings {
        listen: String::new(),
        api_key: None,
        core,
        max_concurrent: 1,
        session_cap: 1,
        max_body_bytes: 1024,
        allow_any_host: false,
        weekly_breaker: false,
        weekly_breaker_threshold: Default::default(),
    });
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let client = reqwest::Client::new();
    let (a, b) = tokio::join!(client.get(&url).send(), client.get(&url).send());
    assert_eq!(a.unwrap().status(), 503);
    assert_eq!(b.unwrap().status(), 503);
    assert_eq!(
        std::fs::read_dir(dir.path())
            .unwrap()
            .filter(|e| e
                .as_ref()
                .unwrap()
                .path()
                .extension()
                .is_some_and(|ext| ext == "log"))
            .count(),
        1
    );
    task.abort();
}
