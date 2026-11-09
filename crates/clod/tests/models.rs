#![cfg(feature = "test-support")]
use clod::{router, Settings};
use clod_core::spawn::Config;
use serde_json::{json, Value};

#[tokio::test]
async fn discovers_models_without_a_user_turn_and_caches_catalog() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("calls.ndjson");
    let scenario = dir.path().join("scenario.json");
    std::fs::write(&scenario, json!({"fresh":{},"log":log}).to_string()).unwrap();
    let mut core = Config::new(dir.path().to_string_lossy());
    core.program = env!("CARGO_BIN_EXE_fake-claude").into();
    // Pass the fake's scenario through the explicit child environment override.
    core.upstream_base_url = Some(scenario.to_string_lossy().into_owned());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let app = router(Settings {
        listen: String::new(),
        api_key: Some("secret".into()),
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
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .unwrap();
    assert_eq!(
        client
            .get(format!("{url}/v1/models"))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let request = |path: &str| client.get(format!("{url}{path}")).bearer_auth("secret");
    // These must fail before spawning even when no catalog has been cached.
    assert_eq!(
        request("/v1/models?limit=0").send().await.unwrap().status(),
        400
    );
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    let (response, concurrent) = tokio::join!(
        request("/v1/models?limit=1").send(),
        request("/v1/models").send(),
    );
    assert_eq!(concurrent.unwrap().status(), 200);
    let response = response.unwrap();
    assert_eq!(response.status(), 200);
    let page: Value = response.json().await.unwrap();
    assert_eq!(page["data"][0]["id"], "sonnet");
    assert_eq!(page["has_more"], true);
    let page: Value = request("/v1/models?after_id=sonnet&limit=1")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(page["data"][0]["id"], "opus");
    assert_eq!(page["has_more"], true);
    let model: Value = request("/v1/models/opus")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(model["display_name"], "Opus");
    assert_eq!(model["resolved_model"], "claude-opus-5-5");
    assert_eq!(model["description"], "Larger model");
    assert_eq!(model["supports_effort"], true);
    assert_eq!(
        model["supported_effort_levels"],
        json!(["low", "medium", "high", "xhigh", "max"])
    );
    assert_eq!(model["supports_adaptive_thinking"], true);
    assert_eq!(model["supports_thinking_off"], false);
    assert_eq!(model["supports_auto_mode"], true);
    assert_eq!(model["supports_fast_mode"], true);
    assert_eq!(
        page["data"][0], model,
        "list and lookup expose identical metadata"
    );
    let resolved: Value = request("/v1/models/claude-opus-5-5")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(resolved["id"], "claude-opus-5-5");
    assert_eq!(
        resolved["supported_effort_levels"],
        model["supported_effort_levels"]
    );
    assert_eq!(
        request("/v1/models/missing").send().await.unwrap().status(),
        404
    );
    for query in [
        "limit=0",
        "limit=1001",
        "limit=no",
        "after_id=missing",
        "after_id=sonnet&before_id=opus",
    ] {
        assert_eq!(
            request(&format!("/v1/models?{query}"))
                .send()
                .await
                .unwrap()
                .status(),
            400
        );
    }
    let logs: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "log"))
        .collect();
    assert_eq!(logs.len(), 1);
    let records: Vec<Value> = std::fs::read_to_string(&logs[0])
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(
        records.iter().filter(|r| r.get("argv").is_some()).count(),
        1,
        "one cached probe"
    );
    assert!(records.iter().all(|r| r["stdin"]["type"] != "user"));
    task.abort();
}
