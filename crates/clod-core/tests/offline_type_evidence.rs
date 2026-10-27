//! Synthetic CLI-boundary evidence only: no process, provider, or CLI execution.
use clod_core::{
    exchange::{Action, Exchange},
    request,
    session::SessionMap,
    spawn::Config,
};
use serde_json::{json, Value};

fn schema() -> Value {
    json!({"type":"object","required":["items","count"],"properties":{
        "items":{"type":"array","items":{"type":"string"}},
        "count":{"type":"number"},"limit":{"type":"integer"}}})
}
fn exchange(stream: bool) -> Exchange {
    let body = json!({"model":"m","max_tokens":64,"stream":stream,
        "messages":[{"role":"user","content":"offline"}],
        "tools":[{"name":"probe","description":"Offline type probe","input_schema":schema()}]});
    let request = request::parse(body.to_string().as_bytes(), &[]).unwrap();
    Exchange::new(request, &SessionMap::default(), Config::new("/empty"), 0).0
}
fn feed(e: &mut Exchange, value: Value) -> Vec<Action> {
    e.on_stdout_line(&value.to_string(), 1)
}
#[test]
fn mcp_schema_preserves_arrays_numbers_and_integer_constraints() {
    let mut e = exchange(true);
    let actions = feed(
        &mut e,
        json!({"type":"control_request","request_id":"offline-list",
        "request":{"subtype":"mcp_message","message":{"jsonrpc":"2.0","id":7,"method":"tools/list"}}}),
    );
    let response: Value = actions
        .iter()
        .find_map(|a| match a {
            Action::WriteStdin(line) => Some(serde_json::from_str(line).unwrap()),
            _ => None,
        })
        .unwrap();
    let tool = &response["response"]["response"]["mcp_response"]["result"]["tools"][0];
    let mut expected = schema();
    expected["$schema"] = json!("http://json-schema.org/draft-07/schema#");
    assert_eq!(tool["inputSchema"], expected);
    assert_eq!(tool["name"], "probe");
    assert_eq!(tool["execution"], json!({"taskSupport":"forbidden"}));
    println!("MCP tools/list response: {tool}");
}
#[test]
fn numeric_and_string_encoded_inputs_cross_cli_boundary_unchanged() {
    // Include lexical whitespace to distinguish byte preservation from reserialization.
    let raw = [
        r#"{ "items": ["a","b"], "count": 2.5, "limit": 3 }"#,
        r#"{ "items": "[\"a\",\"b\"]", "count": "2.5", "limit": "3" }"#,
    ];
    for stream in [true, false] {
        for name in ["probe", "mcp__oc__probe"] {
            let mut e = exchange(stream);
            let mut actions = feed(
                &mut e,
                json!({"type":"stream_event","session_id":"offline",
                "event":{"type":"message_start","message":{"id":"offline","type":"message","role":"assistant","model":"m","content":[]}}}),
            );
            for (index, input) in raw.iter().enumerate() {
                let id = format!("offline_{index}");
                actions.extend(feed(&mut e, json!({"type":"stream_event","event":{"type":"content_block_start","index":index,
                    "content_block":{"type":"tool_use","id":id,"name":name,"input":{}}}})));
                // Fragment inside JSON tokens, as real CLI streamed output may do.
                for part in [&input[..19], &input[19..]] {
                    actions.extend(feed(&mut e, json!({"type":"stream_event","event":{"type":"content_block_delta","index":index,
                        "delta":{"type":"input_json_delta","partial_json":part}}})));
                }
                actions.extend(feed(&mut e, json!({"type":"assistant","uuid":format!("u{index}"),"session_id":"offline",
                    "message":{"id":"offline","role":"assistant","content":[{"type":"tool_use","id":id,"name":name,
                        "input":serde_json::from_str::<Value>(input).unwrap()}]}})));
                actions.extend(feed(&mut e, json!({"type":"stream_event","event":{"type":"content_block_stop","index":index}})));
            }
            actions.extend(feed(&mut e, json!({"type":"stream_event","event":{"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":1}}})));
            actions.extend(feed(
                &mut e,
                json!({"type":"stream_event","event":{"type":"message_stop"}}),
            ));
            actions.extend(feed(
                &mut e,
                json!({"type":"result","subtype":"error_max_turns","is_error":true}),
            ));
            actions.extend(e.on_stdout_eof(2));
            actions.extend(e.on_stderr_eof(2));
            actions.extend(e.on_exit(0, 3));
            if stream {
                for (index, expected) in raw.iter().enumerate() {
                    let actual: String = actions
                        .iter()
                        .filter_map(|a| match a {
                            Action::Emit(event)
                                if event.data["index"] == index
                                    && event.data["delta"]["type"] == "input_json_delta" =>
                            {
                                event.data["delta"]["partial_json"].as_str()
                            }
                            _ => None,
                        })
                        .collect();
                    assert_eq!(actual, *expected);
                }
                let names: Vec<_> = actions
                    .iter()
                    .filter_map(|a| match a {
                        Action::Emit(event) if event.data["type"] == "content_block_start" => {
                            event.data["content_block"]["name"].as_str()
                        }
                        _ => None,
                    })
                    .collect();
                assert_eq!(names, ["probe", "probe"]);
            } else {
                let body = actions
                    .iter()
                    .find_map(|a| match a {
                        Action::RespondJson(200, value) => Some(value),
                        _ => None,
                    })
                    .unwrap();
                for (index, expected) in raw.iter().enumerate() {
                    assert_eq!(
                        body["content"][index]["input"],
                        serde_json::from_str::<Value>(expected).unwrap()
                    );
                    assert_eq!(body["content"][index]["name"], "probe");
                }
            }
            println!("stream={stream} CLI name={name}: typed array/number/integer and string-encoded variants preserved; client name=probe");
        }
    }
}
