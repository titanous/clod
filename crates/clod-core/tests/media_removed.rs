use clod_core::{
    exchange::{Action, Exchange},
    request,
    session::{self, Checkpoint, MapEffect, SessionMap},
    spawn::Config,
};
use serde_json::{json, Value};

fn new(stream: bool, resume: bool) -> Exchange {
    let request = request::parse(&serde_json::to_vec(&json!({"model":"haiku","messages":[{"role":"user","content":"hello"}],"max_tokens":32,"stream":stream,"tools":[{"name":"lookup","input_schema":{}}]})).unwrap(), &[("x-polytoken-conversation", "media")]).unwrap();
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
    let mut config = Config::new("/empty");
    config.turn_timeout_ms = 100;
    Exchange::new(request, &map, config, 0).0
}
fn out(e: &mut Exchange, value: Value) -> Vec<Action> {
    e.on_stdout_line(&value.to_string(), 10)
}
fn notice(camel: bool) -> Value {
    let mut v = json!({"type":"assistant","uuid":"synthetic","session_id":"fork","error":"invalid_request","message":{"content":[{"type":"text","text":"synthetic media notice"}]}});
    v[if camel { "apiError" } else { "api_error" }] = json!("media_removed");
    v[if camel {
        "isApiErrorMessage"
    } else {
        "is_api_error_message"
    }] = json!(true);
    v
}
fn reap(e: &mut Exchange) -> Vec<Action> {
    let mut a = e.on_stdout_eof(110);
    a.extend(e.on_stderr_eof(110));
    a.extend(e.on_exit(0, 110));
    a
}
fn discarded(e: &mut Exchange, actions: &[Action]) {
    assert!(!actions.iter().any(|a| matches!(a, Action::Spawn(_))));
    let finals: Vec<_> = actions
        .iter()
        .filter_map(|a| match a {
            Action::Done(f) => Some(&f.map_effect),
            _ => None,
        })
        .collect();
    assert_eq!(finals, vec![&MapEffect::Discard]);
    assert_eq!(e.checkpoint_reason(), Some("MediaRemoved"));
    assert!(reap(e).is_empty());
}
fn begin(e: &mut Exchange) -> Vec<Action> {
    out(
        e,
        json!({"type":"stream_event","event":{"type":"message_start","message":{"id":"actual","content":[]}}}),
    )
}
fn answer(e: &mut Exchange) -> Vec<Action> {
    out(
        e,
        json!({"type":"assistant","session_id":"fork","uuid":"actual","message":{"content":[{"type":"text","text":"actual answer"}]}}),
    )
}
fn success(e: &mut Exchange) -> Vec<Action> {
    out(
        e,
        json!({"type":"result","subtype":"success","is_error":false}),
    )
}
#[test]
fn repeated_typed_notices_recover_without_synthetic_content() {
    for stream in [false, true] {
        for resume in [false, true] {
            for camel in [false, true] {
                let mut e = new(stream, resume);
                for _ in 0..2 {
                    assert!(out(&mut e, notice(camel)).is_empty());
                }
                let mut a = begin(&mut e);
                a.extend(out(&mut e, json!({"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"text","text":"actual answer"}}})));
                a.extend(answer(&mut e));
                a.extend(success(&mut e));
                assert!(!a.iter().any(|a| matches!(a, Action::RespondJson(500, _))
                    || matches!(a, Action::Emit(event) if event.event == "error")));
                assert!(!format!("{a:?}").contains("synthetic media notice"));
                if !stream {
                    assert!(a.iter().any(|a| matches!(a, Action::RespondJson(200,b) if b["content"][0]["text"] == "actual answer")));
                }
                a.extend(reap(&mut e));
                discarded(&mut e, &a);
            }
        }
    }
}
#[test]
fn failure_exit_timeout_and_disconnect_discard_without_retry() {
    for stream in [false, true] {
        for resume in [false, true] {
            for scenario in [
                "failure",
                "contradiction",
                "invalid_request",
                "stderr",
                "eof",
                "timeout",
                "disconnect",
                "limit",
            ] {
                let mut e = new(stream, resume);
                assert!(out(&mut e, notice(false)).is_empty());
                let mut a = match scenario {
                    "failure" => out(
                        &mut e,
                        json!({"type":"result","subtype":"error_during_execution","is_error":true,"errors":["failed"]}),
                    ),
                    "contradiction" => out(
                        &mut e,
                        json!({"type":"result","subtype":"success","is_error":true,"result":"failed"}),
                    ),
                    "invalid_request" => out(
                        &mut e,
                        json!({"type":"result","subtype":"invalid_request_error","is_error":true}),
                    ),
                    "stderr" => {
                        e.on_stderr_line("No conversation found with session ID: source", 20)
                    }
                    "timeout" => e.on_tick(100),
                    "disconnect" => e.on_client_gone(20),
                    "limit" => {
                        e.on_stdout_line(include_str!("fixtures/live/usage-limit.ndjson"), 20)
                    }
                    _ => vec![],
                };
                a.extend(reap(&mut e));
                assert!(!a.iter().any(|a| matches!(a, Action::RespondJson(200, _))));
                if scenario != "disconnect" {
                    assert!(
                        a.iter()
                            .any(|a| matches!(a, Action::RespondJson(429 | 500, _))),
                        "{scenario}: {a:?}"
                    );
                }
                discarded(&mut e, &a);
            }
        }
    }
}
#[test]
fn stream_failure_after_notice_is_an_error_not_success() {
    let mut e = new(true, true);
    let mut a = begin(&mut e);
    assert!(out(&mut e, notice(true)).is_empty());
    a.extend(out(&mut e,json!({"type":"result","subtype":"error_during_execution","is_error":true,"errors":["failed"]})));
    assert!(a
        .iter()
        .any(|a| matches!(a,Action::Emit(event) if event.event == "error")));
    assert!(!a
        .iter()
        .any(|a| matches!(a,Action::Emit(event) if event.event == "message_stop")));
    a.extend(reap(&mut e));
    discarded(&mut e, &a);
}
#[test]
fn tool_boundary_recovers_but_never_checkpoints() {
    for stream in [false, true] {
        let mut e = new(stream, true);
        assert!(out(&mut e, notice(false)).is_empty());
        let mut a = out(
            &mut e,
            json!({"type":"assistant","uuid":"tool","session_id":"fork","message":{"content":[{"type":"tool_use","id":"call","name":"lookup","input":{}}]}}),
        );
        a.extend(out(
            &mut e,
            json!({"type":"result","subtype":"error_max_turns","is_error":true}),
        ));
        assert!(!a.iter().any(|a| matches!(a, Action::RespondJson(500, _))));
        if !stream {
            assert!(a.iter().any(|a| matches!(a,Action::RespondJson(200,b) if b["stop_reason"] == "tool_use" && b["content"][0]["id"] == "call")));
        }
        a.extend(reap(&mut e));
        discarded(&mut e, &a);
    }
}
#[test]
fn client_note_is_once_after_model_blocks_with_nonoverlapping_indexes() {
    for stream in [false, true] {
        for dimensions in [false, true] {
            for camel in [false, true] {
                let mut e = new(stream, false);
                let mut record = notice(camel);
                if dimensions {
                    let evidence: Value = serde_json::from_str(include_str!(
                        "fixtures/live/media-removed-dimensions.json"
                    ))
                    .unwrap();
                    record[if camel {
                        "errorDetails"
                    } else {
                        "error_details"
                    }] = evidence["errorDetails"].clone();
                    record[if camel {
                        "apiErrorParams"
                    } else {
                        "api_error_params"
                    }] = evidence["apiErrorParams"].clone();
                }
                assert!(out(&mut e, record.clone()).is_empty());
                assert!(out(&mut e, record).is_empty());
                let mut a = begin(&mut e);
                for (index, block) in [
                    (0, json!({"type":"text","text":"actual answer"})),
                    (
                        4,
                        json!({"type":"tool_use","id":"call","name":"lookup","input":{}}),
                    ),
                ] {
                    a.extend(out(&mut e,json!({"type":"stream_event","event":{"type":"content_block_start","index":index,"content_block":block}})));
                    a.extend(out(&mut e,json!({"type":"stream_event","event":{"type":"content_block_stop","index":index}})));
                }
                out(
                    &mut e,
                    json!({"type":"assistant","uuid":"real","session_id":"fork","message":{"content":[{"type":"text","text":"actual answer"},{"type":"tool_use","id":"call","name":"lookup","input":{}}]}}),
                );
                out(
                    &mut e,
                    json!({"type":"stream_event","event":{"type":"message_stop"}}),
                );
                a.extend(success(&mut e));
                if stream {
                    let events: Vec<_> = a
                        .iter()
                        .filter_map(|a| match a {
                            Action::Emit(event) => Some(event),
                            _ => None,
                        })
                        .collect();
                    let starts: Vec<_> = events
                        .iter()
                        .filter(|e| e.event == "content_block_start")
                        .map(|e| e.data["index"].as_u64().unwrap())
                        .collect();
                    assert_eq!(starts, vec![0, 4, 5]);
                    let notes: Vec<_> = events
                        .iter()
                        .filter(|e| e.event == "content_block_delta" && e.data["index"] == 5)
                        .collect();
                    assert_eq!(notes.len(), 1);
                    let note = notes[0].data["delta"]["text"].as_str().unwrap();
                    assert_eq!(note.contains("3000 pixels"), dimensions);
                    assert!(note.contains(if dimensions {
                        "Resize or crop"
                    } else {
                        "could not process"
                    }));
                    let stop = events
                        .iter()
                        .position(|e| e.event == "content_block_stop" && e.data["index"] == 5)
                        .unwrap();
                    let terminal = events
                        .iter()
                        .position(|e| e.event == "message_delta")
                        .unwrap();
                    assert!(stop < terminal);
                    assert_eq!(events[terminal].data["delta"]["stop_reason"], "tool_use");
                } else {
                    let body = a
                        .iter()
                        .find_map(|a| match a {
                            Action::RespondJson(200, b) => Some(b),
                            _ => None,
                        })
                        .unwrap();
                    assert_eq!(body["content"].as_array().unwrap().len(), 3);
                    assert_eq!(body["content"][0]["text"], "actual answer");
                    assert_eq!(body["content"][1]["id"], "call");
                    assert_eq!(body["stop_reason"], "tool_use");
                    assert_eq!(
                        body["content"][2]["text"]
                            .as_str()
                            .unwrap()
                            .contains("3000 pixels"),
                        dimensions
                    );
                }
                a.extend(reap(&mut e));
                discarded(&mut e, &a);
            }
        }
    }
}
#[test]
fn typed_code_alone_preserves_deadline_and_discards_on_stdin_failure() {
    for camel in [false, true] {
        let mut e = new(false, true);
        let mut record = notice(camel);
        record.as_object_mut().unwrap().remove(if camel {
            "isApiErrorMessage"
        } else {
            "is_api_error_message"
        });
        assert!(out(&mut e, record).is_empty());
        assert!(e.on_tick(99).is_empty());
        let mut a = e.on_stdin_failed("broken pipe", 99);
        a.extend(reap(&mut e));
        discarded(&mut e, &a);
    }
}
#[test]
fn reported_dimension_error_metadata_allows_internal_recovery() {
    let evidence: Value =
        serde_json::from_str(include_str!("fixtures/live/media-removed-dimensions.json")).unwrap();
    assert_eq!(evidence["errorDetails"]["status"], 400);
    assert!(evidence["errorDetails"]["message"]
        .as_str()
        .unwrap()
        .ends_with("many-image requests: 3000 pixels"));
    let mut record = notice(true);
    record["apiErrorParams"] = evidence["apiErrorParams"].clone();
    record["errorDetails"] = evidence["errorDetails"].clone();
    let mut e = new(false, true);
    assert!(out(&mut e, record).is_empty());
    answer(&mut e);
    let mut a = success(&mut e);
    assert!(a.iter().any(|a| matches!(a, Action::RespondJson(200, _))));
    a.extend(reap(&mut e));
    discarded(&mut e, &a);
}
#[test]
fn plain_prose_and_nonexact_codes_do_not_trigger_recovery() {
    let mut e = new(false, false);
    out(
        &mut e,
        json!({"type":"assistant","uuid":"real","session_id":"fork","message":{"content":[{"type":"text","text":"media_removed: API Error image removed"}]}}),
    );
    assert!(success(&mut e)
        .iter()
        .any(|a| matches!(a, Action::RespondJson(200, _))));
    assert!(reap(&mut e)
        .iter()
        .any(|a| matches!(a,Action::Done(f) if matches!(f.map_effect,MapEffect::Commit(_)))));
    for code in ["MEDIA_REMOVED", "media_removed_extra", "invalid_request"] {
        let mut e = new(false, false);
        let mut n = notice(true);
        n["apiError"] = json!(code);
        assert!(out(&mut e, n)
            .iter()
            .any(|a| matches!(a, Action::RespondJson(500, _))));
    }
}
