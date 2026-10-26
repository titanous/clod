use clod_core::{
    exchange::{Action, Exchange},
    request,
    session::{MapEffect, SessionMap},
    spawn::Config,
};
use serde_json::json;

#[test]
fn unknown_bare_call_does_not_checkpoint() {
    let req=request::parse(br#"{"model":"m","max_tokens":64,"messages":[{"role":"user","content":"test"}],"tools":[]}"#,&[("x-polytoken-conversation","c")]).unwrap();
    let (mut e, _) = Exchange::new(req, &SessionMap::default(), Config::new("/empty"), 0);
    for line in include_str!("fixtures/live/native-search.ndjson").lines() {
        e.on_stdout_line(line, 1);
    }
    e.on_stdout_eof(2);
    e.on_stderr_eof(3);
    assert!(e
        .on_exit(0, 4)
        .iter()
        .any(|a| matches!(a,Action::Done(f) if f.map_effect==MapEffect::Discard)));
    assert_eq!(e.checkpoint_reason(), Some("MissingUuid"));
}

#[test]
fn mixed_prefixed_and_declared_bare_calls_discard() {
    let req=request::parse(br#"{"model":"m","max_tokens":64,"messages":[{"role":"user","content":"test"}],"tools":[{"name":"lookup","input_schema":{"type":"object"}}]}"#,&[("x-polytoken-conversation","c")]).unwrap();
    let (mut e, _) = Exchange::new(req, &SessionMap::default(), Config::new("/empty"), 0);
    for (id, name, uuid) in [("a", "mcp__oc__lookup", "first"), ("b", "lookup", "last")] {
        e.on_stdout_line(&json!({"type":"assistant","session_id":"s","uuid":uuid,"message":{"content":[{"type":"tool_use","id":id,"name":name,"input":{}}]}}).to_string(),1);
    }
    e.on_stdout_line(
        &json!({"type":"result","subtype":"error_max_turns","is_error":true}).to_string(),
        2,
    );
    e.on_stdout_eof(3);
    e.on_stderr_eof(4);
    let actions = e.on_exit(0, 5);
    assert!(actions
        .iter()
        .any(|a| matches!(a, Action::Done(f) if f.map_effect == MapEffect::Discard)));
    assert_eq!(e.checkpoint_reason(), Some("MixedBareAndPrefixedToolCalls"));
}

#[test]
fn bare_only_turns_keep_checkpoint_safety_gates() {
    for (change, reason) in [
        ("tail", "TrailingContentAfterTool"),
        ("ids", "ToolIdsNotCovered"),
        ("messages", "MultipleMessages"),
        ("unknown", "MissingUuid"),
    ] {
        let req=request::parse(br#"{"model":"m","max_tokens":64,"stream":true,"messages":[{"role":"user","content":"test"}],"tools":[{"name":"lookup","input_schema":{"type":"object"}}]}"#,&[("x-polytoken-conversation","c")]).unwrap();
        let (mut e, _) = Exchange::new(req, &SessionMap::default(), Config::new("/empty"), 0);
        for line in include_str!("fixtures/live/bare-lookup.ndjson").lines() {
            let mut value: serde_json::Value = serde_json::from_str(line).unwrap();
            if change == "messages" && value["type"] == "result" {
                e.on_stdout_line(
                    &json!({"type":"stream_event","event":{"type":"message_start","message":{}}})
                        .to_string(),
                    1,
                );
            }
            if change == "ids" && value["type"] == "assistant" {
                value["message"]["content"][0]["id"] = json!("different");
            }
            if change == "unknown" {
                if value["type"] == "assistant" {
                    value["message"]["content"][0]["name"] = json!("lookup_extra");
                }
                if value["event"]["type"] == "content_block_start" {
                    value["event"]["content_block"]["name"] = json!("lookup_extra");
                }
            }
            if change == "tail" && value["type"] == "result" {
                e.on_stdout_line(&json!({"type":"assistant","session_id":"s","uuid":"tail","message":{"content":[{"type":"text","text":"tail"}]}}).to_string(), 1);
            }
            e.on_stdout_line(&value.to_string(), 1);
        }
        e.on_stdout_eof(2);
        e.on_stderr_eof(3);
        assert!(
            e.on_exit(0, 4)
                .iter()
                .any(|a| matches!(a, Action::Done(f) if f.map_effect == MapEffect::Discard)),
            "{change}"
        );
        assert_eq!(e.checkpoint_reason(), Some(reason), "{change}");
    }
}
