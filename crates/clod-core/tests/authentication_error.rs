use clod_core::{
    exchange::{Action, Exchange},
    protocol::{self, Record},
    request,
    session::{self, Checkpoint, MapEffect, Plan, SessionMap},
    spawn::Config,
};
use serde_json::{json, Value};

const TEXT: &str = "Failed to authenticate: OAuth session expired and could not be refreshed";

// Reported production error metadata and text; identifiers and message are sanitized.
// This is an offline reproduction, not a new live CLI capture.
fn observed() -> Value {
    json!({"type":"assistant","session_id":"sanitized-session","uuid":"sanitized-uuid",
        "error":"authentication_failed","isApiErrorMessage":true,"apiError":null,
        "message":{"id":"sanitized-message","role":"assistant","content":[{"type":"text","text":TEXT}]}})
}

fn new(stream: bool, resume: bool) -> (Exchange, Vec<Action>) {
    let request = request::parse(&serde_json::to_vec(&json!({"model":"haiku","messages":[{"role":"user","content":"hello"}],"max_tokens":32,"stream":stream})).unwrap(), &[("x-polytoken-conversation", "auth-error")]).unwrap();
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
    assert!(matches!(session::plan(&request, &map), Plan::Resume(_)) == resume);
    Exchange::new(request, &map, Config::new("/empty"), 0)
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
fn observed_authentication_metadata_is_preserved() {
    let Record::Assistant(a) = protocol::parse(&observed().to_string()).unwrap() else {
        panic!()
    };
    assert_eq!(a.error.as_deref(), Some("authentication_failed"));
    assert!(a.is_api_error_message);
    assert_eq!(a.api_error, None);
    assert_eq!(a.message["content"][0]["text"], TEXT);
}

#[test]
fn typed_authentication_errors_never_retry_or_commit() {
    for metadata in [
        json!({"error":"authentication_failed","isApiErrorMessage":true,"apiError":null}),
        json!({"error":"authentication_error"}),
        json!({"apiError":"authentication_failed"}),
        json!({"apiError":"authentication_error"}),
        json!({"is_api_error_message":true,"api_error":"authentication_failed"}),
        json!({"api_error":"authentication_error"}),
    ] {
        for stream in [false, true] {
            for content in [false, true] {
                for resume in [false, true] {
                    let (mut e, mut all) = new(stream, resume);
                    if content {
                        all.extend(out(&mut e, json!({"type":"stream_event","event":{"type":"message_start","message":{"id":"msg","content":[]}}})));
                        all.extend(out(&mut e, json!({"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"text","text":"partial"}}})));
                    }
                    let mut record = observed();
                    for field in ["error", "isApiErrorMessage", "apiError"] {
                        record.as_object_mut().unwrap().remove(field);
                    }
                    record
                        .as_object_mut()
                        .unwrap()
                        .extend(metadata.as_object().unwrap().clone());
                    let a = out(&mut e, record);
                    assert_eq!(e.checkpoint_reason(), Some("AssistantApiError"));
                    assert!(a.contains(&Action::CloseStdin));
                    if stream && content {
                        assert!(a.iter().any(|a| matches!(a, Action::Emit(event) if event.event == "error" && event.data["error"]["type"] == "authentication_error" && event.data["error"]["message"] == TEXT)));
                        assert!(!a.iter().any(|a| matches!(a, Action::RespondJson(..))));
                        assert_eq!(
                            all.iter().filter(|a| matches!(a, Action::StartSse)).count(),
                            1
                        );
                    } else {
                        assert!(a.iter().any(|a| matches!(a, Action::RespondJson(401, body) if body["error"]["type"] == "authentication_error" && body["error"]["message"] == TEXT)));
                        assert!(!a.iter().any(|a| matches!(a, Action::StartSse)));
                    }
                    all.extend(a);
                    assert!(out(
                        &mut e,
                        json!({"type":"result","subtype":"success","is_error":false})
                    )
                    .is_empty());
                    all.extend(reap(&mut e));
                    assert_eq!(
                        all.iter().filter(|a| matches!(a, Action::Spawn(_))).count(),
                        1
                    );
                    assert!(!all.iter().any(|a| matches!(a, Action::RateLimited { .. })));
                    assert_eq!(
                        all.iter()
                            .filter(|a| matches!(a, Action::EndResponse))
                            .count(),
                        1
                    );
                    let effects = all
                        .iter()
                        .filter_map(|a| match a {
                            Action::Done(f) => Some(&f.map_effect),
                            _ => None,
                        })
                        .collect::<Vec<_>>();
                    assert_eq!(effects, vec![&MapEffect::Keep]);
                    assert!(reap(&mut e).is_empty());
                }
            }
        }
    }
}

#[test]
fn unknown_typed_errors_remain_generic_even_with_authentication_text() {
    for metadata in [
        json!({"error":"unknown_error"}),
        json!({"apiError":"unknown_error"}),
        json!({"isApiErrorMessage":true,"apiError":null}),
    ] {
        let (mut e, _) = new(false, false);
        let mut record = observed();
        for field in ["error", "isApiErrorMessage", "apiError"] {
            record.as_object_mut().unwrap().remove(field);
        }
        record
            .as_object_mut()
            .unwrap()
            .extend(metadata.as_object().unwrap().clone());
        let a = out(&mut e, record);
        assert!(a.iter().any(|a| matches!(a, Action::RespondJson(500, body) if body["error"]["type"] == "api_error" && body["error"]["message"] == TEXT)));
    }
}

#[test]
fn ordinary_authentication_prose_is_successful() {
    let (mut e, _) = new(false, false);
    let mut record = observed();
    for field in ["error", "isApiErrorMessage", "apiError"] {
        record.as_object_mut().unwrap().remove(field);
    }
    assert!(out(&mut e, record).is_empty());
    let a = out(
        &mut e,
        json!({"type":"result","subtype":"success","is_error":false}),
    );
    assert!(a.iter().any(
        |a| matches!(a, Action::RespondJson(200, body) if body["content"][0]["text"] == TEXT)
    ));
    assert!(reap(&mut e)
        .iter()
        .any(|a| matches!(a, Action::Done(f) if matches!(f.map_effect, MapEffect::Commit(_)))));
}
