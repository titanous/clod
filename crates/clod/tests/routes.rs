use clod::{router, Settings};
use clod_core::spawn::Config;
use serde_json::Value;
#[tokio::test]
async fn shell_routes_and_invalid_body() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let app = router(Settings {
        listen: String::new(),
        api_key: None,
        core: Config::new("unused"),
        max_concurrent: 8,
        session_cap: 256,
        max_body_bytes: 256 * 1024 * 1024,
        allow_any_host: false,
    });
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let client = reqwest::Client::new();
    assert_eq!(
        client
            .get(format!("{url}/health"))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
        "ok"
    );
    let models: Value = client
        .get(format!("{url}/v1/models"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!models["data"].as_array().unwrap().is_empty());
    for path in ["/missing", "/v1/messages"] {
        let response = client.get(format!("{url}{path}")).send().await.unwrap();
        assert_eq!(response.status(), 404);
        assert_eq!(response.json::<Value>().await.unwrap()["type"], "error");
    }
    let response = client
        .post(format!("{url}/v1/messages"))
        .body("invalid")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    assert_eq!(
        response.json::<Value>().await.unwrap()["error"]["type"],
        "invalid_request_error"
    );
    for origin in ["null", "http://localhost", "https://evil.test"] {
        let response = client
            .get(format!("{url}/health"))
            .header("origin", origin)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 403);
        assert_eq!(response.json::<Value>().await.unwrap()["type"], "error");
    }
    for host in ["evil.test", "localhost.evil", "127.0.0.2"] {
        assert_eq!(
            client
                .get(format!("{url}/health"))
                .header("host", host)
                .send()
                .await
                .unwrap()
                .status(),
            403
        );
    }
    for host in ["localhost:42", "127.0.0.1:42", "[::1]:42"] {
        assert_eq!(
            client
                .get(format!("{url}/health"))
                .header("host", host)
                .send()
                .await
                .unwrap()
                .status(),
            200
        );
    }
    // Valid large JSON reaches request parsing rather than axum's old 2 MiB limit.
    let large = serde_json::json!({"padding":"x".repeat(3*1024*1024)});
    let response = client
        .post(format!("{url}/v1/messages"))
        .json(&large)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    assert!(response.text().await.unwrap().contains("missing field"));
    for name in ["a,b", "a b"] {
        let response=client.post(format!("{url}/v1/messages")).json(&serde_json::json!({"model":"m","max_tokens":1,"messages":[],"tools":[{"name":name}]})).send().await.unwrap();
        assert_eq!(response.status(), 400);
    }
    task.abort();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let app = router(Settings {
        listen: String::new(),
        api_key: None,
        core: Config::new("unused"),
        max_concurrent: 1,
        session_cap: 1,
        max_body_bytes: 64,
        allow_any_host: true,
    });
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    assert_eq!(
        client
            .get(format!("{url}/health"))
            .header("host", "other.test")
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(
        client
            .get(format!("{url}/health"))
            .header("origin", "null")
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    let reply = client
        .post(format!("{url}/v1/messages"))
        .body("x".repeat(65))
        .send()
        .await
        .unwrap();
    assert_eq!(reply.status(), 400);
    assert_eq!(reply.json::<Value>().await.unwrap()["type"], "error");
    task.abort();
}
