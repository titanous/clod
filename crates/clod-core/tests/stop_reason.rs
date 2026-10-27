use clod_core::{
    exchange::{Action, Exchange},
    request,
    session::SessionMap,
    spawn::Config,
};
use serde_json::{json, Value};

fn final_reason(stream: bool, records: Vec<Value>, tools: bool) -> Value {
    let request = request::parse(
        &serde_json::to_vec(&json!({
            "model":"haiku", "messages":[{"role":"user","content":"hello"}],
            "max_tokens":2048, "stream":stream,
            "tools":if tools {json!([{"name":"lookup","input_schema":{"type":"object"}}])} else {json!([])}
        })).unwrap(),
        &[],
    ).unwrap();
    let (mut exchange, _) =
        Exchange::new(request, &SessionMap::default(), Config::new("/empty"), 0);
    for record in records {
        exchange.on_stdout_line(&record.to_string(), 1);
    }
    let actions = exchange.on_stdout_line(
        &json!({"type":"result","subtype":if tools {"error_max_turns"} else {"success"},"usage":{}}).to_string(),
        2,
    );
    actions
        .into_iter()
        .find_map(|action| match action {
            Action::RespondJson(200, message) if !stream => Some(message["stop_reason"].clone()),
            Action::Emit(event) if stream && event.event == "message_delta" => {
                Some(event.data["delta"]["stop_reason"].clone())
            }
            _ => None,
        })
        .expect("successful terminal response")
}

fn assistant(content: Value, reason: Value) -> Value {
    json!({"type":"assistant","uuid":"assistant","session_id":"session","message":{"content":[content],"stop_reason":reason}})
}

fn delta(reason: Value) -> Value {
    json!({"type":"stream_event","event":{"type":"message_delta","delta":{"stop_reason":reason,"stop_sequence":null},"usage":{"output_tokens":1}}})
}

#[test]
fn thinking_only_assistant_record_without_message_delta() {
    for stream in [false, true] {
        assert_eq!(
            final_reason(
                stream,
                vec![assistant(
                    json!({"type":"thinking","thinking":"consider","signature":"signed"}),
                    json!("end_turn")
                )],
                false
            ),
            "end_turn"
        );
    }
}

#[test]
fn text_only_assistant_record_without_message_delta() {
    for stream in [false, true] {
        assert_eq!(
            final_reason(
                stream,
                vec![assistant(
                    json!({"type":"text","text":"OK"}),
                    json!("end_turn")
                )],
                false
            ),
            "end_turn"
        );
    }
}

#[test]
fn tool_turn_without_message_delta() {
    for stream in [false, true] {
        assert_eq!(
            final_reason(
                stream,
                vec![assistant(
                    json!({"type":"tool_use","id":"tool","name":"mcp__oc__lookup","input":{}}),
                    Value::Null
                )],
                true
            ),
            "tool_use"
        );
    }
}

#[test]
fn null_terminal_reason_uses_last_assistant_reason() {
    for stream in [false, true] {
        assert_eq!(
            final_reason(
                stream,
                vec![
                    assistant(
                        json!({"type":"thinking","thinking":"consider"}),
                        json!("max_tokens")
                    ),
                    assistant(json!({"type":"text","text":"OK"}), json!("end_turn")),
                    delta(Value::Null),
                ],
                false
            ),
            "end_turn"
        );
    }
}

#[test]
fn missing_terminal_reason_uses_assistant_reason() {
    for stream in [false, true] {
        assert_eq!(
            final_reason(
                stream,
                vec![assistant(
                    json!({"type":"text","text":"partial"}),
                    json!("max_tokens")
                )],
                false
            ),
            "max_tokens"
        );
    }
}

#[test]
fn invalid_last_assistant_reason_defaults_to_end_turn() {
    for stream in [false, true] {
        for reason in [Value::Null, json!(7)] {
            assert_eq!(
                final_reason(
                    stream,
                    vec![
                        assistant(json!({"type":"text","text":"earlier"}), json!("max_tokens")),
                        assistant(json!({"type":"text","text":"OK"}), reason),
                        delta(Value::Null),
                    ],
                    false
                ),
                "end_turn"
            );
        }
    }
}

#[test]
fn forwarded_tools_preserve_existing_terminal_override() {
    for stream in [false, true] {
        assert_eq!(
            final_reason(
                stream,
                vec![
                    assistant(
                        json!({"type":"tool_use","id":"tool","name":"mcp__oc__lookup","input":{}}),
                        json!("end_turn")
                    ),
                    delta(json!("end_turn"))
                ],
                true
            ),
            "tool_use"
        );
    }
}

#[test]
fn no_terminal_or_assistant_reason_defaults_to_end_turn() {
    for stream in [false, true] {
        assert_eq!(
            final_reason(
                stream,
                vec![assistant(json!({"type":"text","text":"OK"}), Value::Null)],
                false
            ),
            "end_turn"
        );
    }
}

#[test]
fn terminal_reason_takes_precedence_over_assistant_reason() {
    for stream in [false, true] {
        assert_eq!(
            final_reason(
                stream,
                vec![
                    assistant(json!({"type":"text","text":"OK"}), json!("end_turn")),
                    delta(json!("max_tokens"))
                ],
                false
            ),
            "max_tokens"
        );
    }
}
