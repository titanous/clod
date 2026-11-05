use clod_core::{
    exchange::{Action, Exchange},
    protocol::{self, Record},
    request,
    session::{self, Checkpoint, MapEffect, SessionMap},
    spawn::Config,
};
use serde_json::{json, Value};

const LIMIT: &str = include_str!("fixtures/live/usage-limit.ndjson");

fn new(stream: bool, resume: bool) -> Exchange {
    let request = request::parse(&serde_json::to_vec(&json!({"model":"haiku","messages":[{"role":"user","content":"hello"}],"max_tokens":32,"stream":stream,"tools":[{"name":"lookup","input_schema":{}}]})).unwrap(), &[("x-polytoken-conversation", "usage-limit")]).unwrap();
    let mut map = SessionMap::default();
    if resume {
        map.insert(
            session::key(&request).unwrap(),
            Checkpoint {
                session_id: "source".into(),
                resume_uuid: "old".into(),
                prefix_hash: session::prefix_hash(&[]),
                pending_tool_ids: vec![],
            },
        );
    }
    Exchange::new(request, &map, Config::new("/empty"), 0).0
}
fn out(e: &mut Exchange, value: Value) -> Vec<Action> {
    e.on_stdout_line(&value.to_string(), 10)
}
fn reap(e: &mut Exchange) -> Vec<Action> {
    let mut a = e.on_stdout_eof(20);
    a.extend(e.on_stderr_eof(20));
    a.extend(e.on_exit(0, 20));
    a
}

#[test]
fn terminal_rate_limit_emits_one_typed_diagnostic() {
    for stream in [false, true] {
        let mut e = new(stream, false);
        if stream {
            out(
                &mut e,
                json!({"type":"stream_event","event":{"type":"message_start","message":{"id":"msg","content":[]}}}),
            );
            out(
                &mut e,
                json!({"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"text","text":"partial"}}}),
            );
        }
        let actions = out(
            &mut e,
            json!({"type":"result","subtype":"error_during_execution","is_error":true,"errors":["usage limit reached"]}),
        );
        assert_eq!(
            actions
                .iter()
                .filter(|a| matches!(a, Action::RateLimited { .. }))
                .count(),
            1
        );
        assert!(actions.contains(&Action::RateLimited {
            http_status: if stream { 200 } else { 429 }
        }));
    }
}

#[test]
fn captured_error_metadata_is_preserved() {
    let Record::Assistant(a) = protocol::parse(LIMIT).unwrap() else {
        panic!()
    };
    assert_eq!(a.error.as_deref(), Some("rate_limit"));
    assert!(a.is_api_error_message);
    assert_eq!(a.api_error.as_deref(), Some("usage_limit_reached"));
}

#[test]
fn usage_limit_before_and_after_content_never_commits() {
    for stream in [false, true] {
        for content in [false, true] {
            for resume in [false, true] {
                let mut e = new(stream, resume);
                if content {
                    out(
                        &mut e,
                        json!({"type":"stream_event","event":{"type":"message_start","message":{"id":"msg","content":[]}}}),
                    );
                    out(
                        &mut e,
                        json!({"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"text","text":"partial"}}}),
                    );
                }
                let mut a = e.on_stdout_line(LIMIT, 10);
                assert_eq!(
                    a.iter()
                        .filter(|a| matches!(a, Action::RateLimited { .. }))
                        .count(),
                    1
                );
                assert!(a.contains(&Action::RateLimited {
                    http_status: if stream && content { 200 } else { 429 }
                }));
                assert_eq!(e.checkpoint_reason(), Some("AssistantApiError"));
                assert!(a.iter().any(|a| matches!(a, Action::CloseStdin)));
                if stream && content {
                    assert!(a.iter().any(|a| matches!(a, Action::Emit(event) if event.event == "error" && event.data["error"]["type"] == "rate_limit_error")));
                    assert!(!a.iter().any(|a| matches!(a, Action::RespondJson(..))));
                } else {
                    assert!(a.iter().any(|a| matches!(a, Action::RespondJson(429, body) if body["error"]["type"] == "rate_limit_error" && body["error"]["message"].as_str().unwrap().contains("5-hour limit"))));
                    assert!(!a.iter().any(|a| matches!(a, Action::StartSse)));
                }
                assert!(out(
                    &mut e,
                    json!({"type":"result","subtype":"success","is_error":false})
                )
                .is_empty());
                a.extend(reap(&mut e));
                assert_eq!(
                    a.iter()
                        .filter(|a| matches!(a, Action::EndResponse))
                        .count(),
                    1
                );
                let done = a
                    .iter()
                    .filter_map(|a| {
                        if let Action::Done(f) = a {
                            Some(&f.map_effect)
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>();
                assert_eq!(done, vec![&MapEffect::Keep]);
                assert!(reap(&mut e).is_empty());
            }
        }
    }
}

#[test]
fn ordinary_limit_prose_is_successful() {
    let mut e = new(false, false);
    let mut record: Value = serde_json::from_str(LIMIT).unwrap();
    for field in ["error", "isApiErrorMessage", "apiError"] {
        record.as_object_mut().unwrap().remove(field);
    }
    assert!(out(&mut e, record).is_empty());
    let a = out(
        &mut e,
        json!({"type":"result","subtype":"success","is_error":false}),
    );
    assert!(a.iter().any(|a| matches!(a, Action::RespondJson(200, body) if body["content"][0]["text"].as_str().unwrap().contains("5-hour limit"))));
    assert!(reap(&mut e)
        .iter()
        .any(|a| matches!(a, Action::Done(f) if matches!(f.map_effect, MapEffect::Commit(_)))));
}

#[test]
fn metadata_alone_detects_errors_and_bare_tools_discard() {
    for metadata in [
        json!({"error":"rate_limit"}),
        json!({"apiError":"usage_limit_reached"}),
        json!({"isApiErrorMessage":true}),
        json!({"is_api_error_message":true,"api_error":"usage_limit_reached"}),
    ] {
        let mut e = new(false, false);
        let mut record = json!({"type":"assistant","message":{"content":[]}});
        record
            .as_object_mut()
            .unwrap()
            .extend(metadata.as_object().unwrap().clone());
        let a = out(&mut e, record);
        assert!(a
            .iter()
            .any(|a| matches!(a, Action::RespondJson(429 | 500, _))));
        assert!(reap(&mut e)
            .iter()
            .any(|a| matches!(a, Action::Done(f) if f.map_effect == MapEffect::Keep)));
    }
    let mut e = new(true, true);
    // A declared bare call follows the existing unsuccessful-turn discard rule.
    out(
        &mut e,
        json!({"type":"stream_event","event":{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"bare","name":"lookup","input":{}}}}),
    );
    e.on_stdout_line(LIMIT, 10);
    assert!(reap(&mut e)
        .iter()
        .any(|a| matches!(a, Action::Done(f) if f.map_effect == MapEffect::Discard)));
}

#[test]
fn contradictory_terminal_success_is_an_error() {
    for stream in [false, true] {
        let mut e = new(stream, true);
        let a = out(
            &mut e,
            json!({"type":"result","subtype":"success","is_error":true,"result":"usage limit reached"}),
        );
        assert!(a.iter().any(|a| matches!(a, Action::RespondJson(429, _))));
        assert!(!a.iter().any(|a| matches!(a, Action::StartSse)));
        assert!(reap(&mut e)
            .iter()
            .any(|a| matches!(a, Action::Done(f) if f.map_effect == MapEffect::Keep)));
    }
}
