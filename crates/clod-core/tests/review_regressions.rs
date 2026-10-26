use clod_core::{
    exchange::{Action, Exchange},
    request,
    session::{self, Checkpoint, MapEffect, SessionMap},
    spawn::Config,
};
use serde_json::{json, Value};
#[test]
fn streamed_bare_name_without_assistant_uuid_discards() {
    let mut e = new(false, true);
    event(&mut e, json!({"type":"message_start","message":{}}));
    let actions = event(
        &mut e,
        json!({"type":"content_block_start","content_block":{"type":"tool_use","id":"a","name":"lookup","input":{}}}),
    );
    assert!(actions
        .iter()
        .any(|a| matches!(a,Action::Emit(v) if v.data["content_block"]["name"]=="lookup")));
    event(&mut e, json!({"type":"message_stop"}));
    discarded(&mut e, "MissingUuid");
}

#[test]
fn stale_result_after_disconnect_discards() {
    let mut e = new(true, false);
    e.on_client_gone(1);
    send(
        &mut e,
        json!({"type":"result","subtype":"error_during_execution","errors":["No message found with message.uuid of: u"]}),
    );
    assert!(finish(&mut e)
        .iter()
        .any(|a| matches!(a,Action::Done(f) if f.map_effect==MapEffect::Discard)));
}

fn new(resume: bool, stream: bool) -> Exchange {
    let r=request::parse(&serde_json::to_vec(&json!({"model":"m","max_tokens":64,"stream":stream,"messages":[{"role":"assistant","content":"old"},{"role":"user","content":"next"}],"tools":[{"name":"lookup","input_schema":{"type":"object"}}]})).unwrap(),&[("x-polytoken-conversation","c")]).unwrap();
    let mut map = SessionMap::default();
    if resume {
        map.insert(
            session::key(&r).unwrap(),
            Checkpoint {
                session_id: "s".into(),
                resume_uuid: "u".into(),
                pending_tool_ids: vec![],
                prefix_hash: session::prefix_hash(&r.messages[..1]),
            },
        );
    }
    let mut config = Config::new("/empty");
    config.turn_timeout_ms = 100;
    Exchange::new(r, &map, config, 0).0
}
fn send(e: &mut Exchange, v: Value) -> Vec<Action> {
    e.on_stdout_line(&v.to_string(), 1)
}
fn event(e: &mut Exchange, v: Value) -> Vec<Action> {
    send(e, json!({"type":"stream_event","event":v}))
}
fn assistant(e: &mut Exchange, tool: bool) {
    send(
        e,
        json!({"type":"assistant","uuid":if tool{"tool"}else{"text"},"session_id":"s","message":{"content":if tool{json!([{"type":"tool_use","name":"mcp__oc__lookup","id":"a","input":{}}])}else{json!([{"type":"text","text":"tail"}])}}}),
    );
}
fn finish(e: &mut Exchange) -> Vec<Action> {
    e.on_stdout_eof(3);
    e.on_stderr_eof(4);
    e.on_exit(0, 5)
}
fn discarded(e: &mut Exchange, reason: &str) {
    send(e, json!({"type":"result","subtype":"success"}));
    assert_eq!(e.checkpoint_reason(), Some(reason));
    assert!(finish(e)
        .iter()
        .any(|a| matches!(a,Action::Done(f) if f.map_effect==MapEffect::Discard)));
}
#[test]
fn trailing_assistant_content_discards() {
    let mut e = new(false, false);
    assistant(&mut e, true);
    assistant(&mut e, false);
    discarded(&mut e, "TrailingContentAfterTool");
}
#[test]
fn streamed_and_recorded_ids_must_be_equal() {
    let mut e = new(false, true);
    event(&mut e, json!({"type":"message_start","message":{}}));
    assistant(&mut e, true);
    event(&mut e, json!({"type":"message_stop"}));
    discarded(&mut e, "ToolIdsNotCovered");
}
#[test]
fn second_message_suppresses_blocks_and_records() {
    let mut e = new(false, false);
    event(&mut e, json!({"type":"message_start","message":{}}));
    let a = event(&mut e, json!({"type":"message_start","message":{}}));
    assert!(a.iter().any(|a| matches!(a, Action::Warning(_))));
    assert!(event(
        &mut e,
        json!({"type":"content_block_start","content_block":{"type":"text","text":"forbidden"}})
    )
    .is_empty());
    assistant(&mut e, false);
    let a = send(&mut e, json!({"type":"result","subtype":"success"}));
    let body = a
        .iter()
        .find_map(|a| match a {
            Action::RespondJson(_, v) => Some(v),
            _ => None,
        })
        .unwrap();
    assert_eq!(body["content"], json!([]));
    assert_eq!(e.checkpoint_reason(), Some("MultipleMessages"));
    assert!(finish(&mut e)
        .iter()
        .any(|a| matches!(a,Action::Done(f) if f.map_effect==MapEffect::Discard)));
}
#[test]
fn invalid_resume_retries_once_and_keeps_deadline() {
    for by_text in [false, true] {
        let mut e = new(true, false);
        let a = send(
            &mut e,
            if by_text {
                json!({"type":"result","subtype":"error_during_execution","errors":["invalid_request_error: bad history"]})
            } else {
                json!({"type":"result","subtype":"invalid_request_error"})
            },
        );
        assert!(!a.iter().any(|a| matches!(a, Action::RespondJson(..))));
        assert!(finish(&mut e).iter().any(|a| matches!(a, Action::Spawn(_))));
        let a = e.on_tick(100);
        assert!(a
            .iter()
            .any(|a| matches!(a,Action::RespondJson(_,v) if v.to_string().contains("timed out"))));
    }
}
#[test]
fn invalid_resume_after_commit_errors_and_discards() {
    let mut e = new(true, true);
    event(&mut e, json!({"type":"message_start","message":{}}));
    let a = send(
        &mut e,
        json!({"type":"result","subtype":"invalid_request_error"}),
    );
    assert!(a
        .iter()
        .any(|a| matches!(a,Action::Emit(v) if v.event=="error")));
    assert!(a.iter().any(|a| matches!(a, Action::EndResponse)));
    assert!(finish(&mut e)
        .iter()
        .any(|a| matches!(a,Action::Done(f) if f.map_effect==MapEffect::Discard)));
}
#[test]
fn late_stale_overrides_cancel_keep() {
    for timeout in [false, true] {
        let mut e = new(true, false);
        if timeout {
            e.on_tick(100);
        } else {
            e.on_client_gone(1);
        }
        e.on_stderr_line("No conversation found with session ID: s", 101);
        assert!(finish(&mut e)
            .iter()
            .any(|a| matches!(a,Action::Done(f) if f.map_effect==MapEffect::Discard)));
    }
}
#[test]
fn end_response_precedes_process_cleanup() {
    let mut e = new(false, false);
    assistant(&mut e, false);
    let a = send(&mut e, json!({"type":"result","subtype":"success"}));
    assert!(matches!(a[0], Action::RespondJson(200, _)));
    assert!(matches!(a[1], Action::EndResponse));
    assert!(matches!(a[2], Action::CloseStdin));
    assert!(!a.iter().any(|a| matches!(a, Action::Done(_))));
    assert_eq!(
        finish(&mut e)
            .iter()
            .filter(|a| matches!(a, Action::Done(_)))
            .count(),
        1
    );
    assert!(e.on_exit(0, 6).is_empty());
}
#[test]
fn canonical_bytes_are_sorted() {
    let v: Value = serde_json::from_str(r#"{"z":{"y":1,"a":2},"a":0}"#).unwrap();
    assert_eq!(session::canonical_json(&v), br#"{"a":0,"z":{"a":2,"y":1}}"#);
}
#[test]
fn comma_and_whitespace_tool_names_rejected() {
    for name in ["a,b", "a b", "a\tb", "a\nb", "a\u{2003}b"] {
        let body = json!({"model":"m","max_tokens":1,"messages":[],"tools":[{"name":name}]});
        assert!(request::parse(&serde_json::to_vec(&body).unwrap(), &[]).is_err());
    }
}
