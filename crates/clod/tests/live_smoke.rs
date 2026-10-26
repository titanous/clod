//! Explicitly operator-authorized subscription test. Never run in offline checks.
use serde_json::{json, Value};
use std::{process::Stdio, time::Duration};
use tokio::io::AsyncBufReadExt;

#[tokio::test]
#[ignore = "real subscription calls; requires explicit CLOD_LIVE=1"]
async fn live_smoke() {
    assert_eq!(
        std::env::var("CLOD_LIVE").as_deref(),
        Ok("1"),
        "refusing live calls without CLOD_LIVE=1"
    );
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("measure.ndjson");
    let work = dir.path().join("workdir");
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_clod"))
        .env("CLOD_LISTEN", "127.0.0.1:0")
        .env("CLOD_WORKDIR", &work)
        .env("CLOD_MEASURE_LOG", &log)
        .env(
            "CLOD_CLAUDE_BIN",
            std::env::var("CLOD_CLAUDE_BIN").unwrap_or_else(|_| "claude".into()),
        )
        .env("RUST_LOG", "info")
        .env_remove("CLOD_API_KEY")
        .env_remove("CLOD_UPSTREAM_BASE_URL")
        .env_remove("CLOD_SESSION_KEY")
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut lines = tokio::io::BufReader::new(child.stdout.take().unwrap()).lines();
    let address = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let line = lines.next_line().await.unwrap().expect("clod exited");
            if let Some((_, address)) = line.split_once("address=") {
                break address.trim().to_owned();
            }
        }
    })
    .await
    .unwrap();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(180))
        .build()
        .unwrap();
    let mut body = json!({"model":"haiku","max_tokens":2048,"stream":false,"system":"Follow the user's requested exact tool rounds. Use the exposed mcp__oc__ tools. Never invent tool results.","messages":[{"role":"user","content":"Remember the marker smoke-cobalt. Reply only READY, no tools."}],"tools":[{"name":"lookup","description":"Return a test value for a key","input_schema":{"type":"object","properties":{"key":{"type":"string"}},"required":["key"]}},{"name":"combine","description":"Combine remembered test values","input_schema":{"type":"object","properties":{"values":{"type":"array","items":{"type":"string"}}},"required":["values"]}}]});
    let mut values = Vec::new();
    for turn in 0..5 {
        let reply = client
            .post(format!("http://{address}/v1/messages"))
            .header("x-polytoken-conversation", "clod-live-smoke")
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = reply.status();
        let text = reply.text().await.unwrap();
        assert!(status.is_success(), "{status}: {text}");
        assert!(!text.to_ascii_lowercase().contains("missing-message"));
        assert!(!text.contains("No message found"));
        let reply: Value = serde_json::from_str(&text).unwrap();
        let blocks = reply["content"].as_array().unwrap();
        body["messages"]
            .as_array_mut()
            .unwrap()
            .push(json!({"role":"assistant","content":blocks}));
        let prompt=match turn {
            0=>json!("What marker did I give you? Reply with it, no tools."),
            1=>json!("Now call lookup exactly three times in parallel, keys alpha, beta, gamma. Do not call combine yet."),
            2=>{let tools:Vec<_>=blocks.iter().filter(|b|b["type"]=="tool_use").collect();assert_eq!(tools.len(),3,"expected three parallel calls: {reply}");let mut results=Vec::new();for tool in tools {assert_eq!(tool["name"],"lookup");let key=tool["input"]["key"].as_str().unwrap();let value=format!("VALUE_{key}_ROUND_ONE");values.push(value.clone());results.push(json!({"type":"tool_result","tool_use_id":tool["id"],"content":value}));}results.push(json!({"type":"text","text":"Now call combine once with all three returned values. Do not answer yet."}));Value::Array(results)},
            3=>{let tools:Vec<_>=blocks.iter().filter(|b|b["type"]=="tool_use").collect();assert_eq!(tools.len(),1,"expected second round: {reply}");assert_eq!(tools[0]["name"],"combine");values.push("VALUE_COMBINED_ROUND_TWO".into());json!([{"type":"tool_result","tool_use_id":tools[0]["id"],"content":"VALUE_COMBINED_ROUND_TWO"},{"type":"text","text":"Final answer: quote every returned value from both rounds and the original marker; no more tools."}])},
            _=>{let answer=blocks.iter().filter_map(|b|b["text"].as_str()).collect::<Vec<_>>().join(" ");for value in &values {assert!(answer.contains(value),"missing {value}: {answer}");}assert!(answer.contains("smoke-cobalt"));Value::Null}
        };
        if turn < 4 {
            body["messages"]
                .as_array_mut()
                .unwrap()
                .push(json!({"role":"user","content":prompt}));
        }
    }
    let records: Vec<Value> = std::fs::read_to_string(&log)
        .unwrap()
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    let spawns: Vec<_> = records.iter().filter(|v| v["kind"] == "spawn").collect();
    assert_eq!(spawns.len(), 5, "unexpected retries: {spawns:?}");
    assert_eq!(spawns[0]["plan"], "fresh");
    for spawn in &spawns[1..] {
        assert_eq!(spawn["plan"], "resume");
        let request = &spawn["request"];
        assert!(
            records.iter().any(|v| v["kind"] == "usage"
                && &v["request"] == request
                && v["event"] == "message_start"
                && v["usage"]["cache_read_input_tokens"].as_u64().unwrap_or(0) > 0),
            "no cache read for {request}"
        );
    }
    eprintln!("Dedicated CLI transcripts retained under ~/.claude/projects for workdir {} (operator cleanup)",work.display());
    child.kill().await.unwrap();
}
