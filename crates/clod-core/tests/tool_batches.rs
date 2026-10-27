//! Multi-call tool batches: typed inputs, names and IDs through the client
//! response and the next turn's resume or replay.
//!
//! The interleaved shape mirrors a captured CLI transcript (Claude CLI 2.1.308):
//! for a bare tool name the CLI has no tool of that name, so while the message
//! is still streaming it writes a `No such tool available` tool_result as the
//! child of the first tool record, and the next tool block of the *same*
//! upstream message is parented to that error. Resuming at the last tool
//! record therefore keeps the earlier error on the resumed chain, and the
//! CLI's upstream request carried that error instead of the client's real
//! result for every call except the last.
use clod_core::{
    exchange::{Action, Exchange},
    request::{self, Request},
    session::{self, MapEffect, Plan, SessionMap},
    spawn::Config,
};
use serde_json::{json, Value};

const PREFIX: &str = "mcp__oc__";

fn request_body(stream: bool, messages: Value) -> Request {
    let body = json!({
        "model": "m",
        "max_tokens": 64,
        "stream": stream,
        "messages": messages,
        "tools": [
            {"name": "progress_update", "input_schema": {"type": "object", "properties": {
                "phase": {"type": "string"},
                "completed": {"type": "array", "items": {"type": "string"}},
                "remaining": {"type": "array", "items": {"type": "string"}}}}},
            {"name": "exit_tool", "input_schema": {"type": "object", "properties": {
                "summary": {"type": "string"},
                "files": {"type": "array", "items": {"type": "string"}}}}},
            {"name": "file_edit_search_replace", "input_schema": {"type": "object", "properties": {
                "path": {"type": "string"},
                "old_string": {"type": "string"},
                "new_string": {"type": "string"}}}}
        ]
    });
    request::parse(
        body.to_string().as_bytes(),
        &[("x-polytoken-conversation", "batch")],
    )
    .unwrap()
}

fn first_user() -> Value {
    json!([{"role": "user", "content": "do the batch"}])
}

/// The calls in one assistant message, with typed array and scalar inputs.
fn calls() -> Vec<(&'static str, &'static str, Value)> {
    vec![
        (
            "toolu_a",
            "progress_update",
            json!({"phase": "investigating", "completed": ["read exchange.rs", "ran \"tests\""], "remaining": []}),
        ),
        (
            "toolu_b",
            "file_edit_search_replace",
            json!({"path": "src/lib.rs", "old_string": "a\n\"b\"", "new_string": "[\"not\", \"an array\"]"}),
        ),
        (
            "toolu_c",
            "exit_tool",
            json!({"summary": "done", "files": ["a.rs", "b/c.rs"]}),
        ),
    ]
}

#[derive(Clone, Copy, PartialEq)]
enum Shape {
    /// Prefixed names: hooks are held, so CLI results follow every tool record.
    Prefixed,
    /// Bare names with each CLI error emitted before the next tool record (observed).
    BareInterleaved,
    /// Bare names with all CLI errors after the last tool record.
    BareTrailing,
}

/// Splits `text` into three fragments so the client must reassemble deltas.
fn fragments(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let (a, b) = (chars.len() / 3, 2 * chars.len() / 3);
    vec![
        chars[..a].iter().collect(),
        chars[a..b].iter().collect(),
        chars[b..].iter().collect(),
    ]
}

fn cli_error(id: &str, name: &str, uuid: &str) -> Value {
    json!({"type": "user", "uuid": uuid, "session_id": "s", "parent_tool_use_id": null,
        "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": id, "is_error": true,
            "content": format!("<tool_use_error>Error: No such tool available: {name}</tool_use_error>")}]}})
}

fn stdout(calls: &[(&str, &str, Value)], raw_inputs: &[String], shape: Shape) -> Vec<Value> {
    let mut lines = vec![
        json!({"type": "stream_event", "session_id": "s", "event": {"type": "message_start",
            "message": {"id": "msg", "type": "message", "role": "assistant", "model": "m", "content": []}}}),
    ];
    let wire = |name: &str| {
        if shape == Shape::Prefixed {
            format!("{PREFIX}{name}")
        } else {
            name.to_string()
        }
    };
    for (index, ((id, name, input), raw)) in calls.iter().zip(raw_inputs).enumerate() {
        lines.push(json!({"type": "stream_event", "session_id": "s", "event": {"type": "content_block_start", "index": index,
            "content_block": {"type": "tool_use", "id": id, "name": wire(name), "input": {}}}}));
        lines.push(json!({"type": "stream_event", "session_id": "s", "event": {"type": "content_block_delta", "index": index,
            "delta": {"type": "input_json_delta", "partial_json": ""}}}));
        for part in fragments(raw) {
            lines.push(json!({"type": "stream_event", "session_id": "s", "event": {"type": "content_block_delta", "index": index,
                "delta": {"type": "input_json_delta", "partial_json": part}}}));
        }
        lines.push(
            json!({"type": "assistant", "uuid": format!("u{index}"), "session_id": "s",
            "message": {"id": "msg", "role": "assistant", "stop_reason": "tool_use",
                "content": [{"type": "tool_use", "id": id, "name": wire(name), "input": input}]}}),
        );
        if shape == Shape::BareInterleaved {
            lines.push(cli_error(id, name, &format!("e{index}")));
        }
        lines.push(json!({"type": "stream_event", "session_id": "s", "event": {"type": "content_block_stop", "index": index}}));
    }
    lines.push(json!({"type": "stream_event", "session_id": "s", "event": {"type": "message_delta",
        "delta": {"type": "message_delta", "stop_reason": "tool_use", "stop_sequence": null}, "usage": {"output_tokens": 9}}}));
    lines.push(
        json!({"type": "stream_event", "session_id": "s", "event": {"type": "message_stop"}}),
    );
    if shape == Shape::BareTrailing {
        for (index, (id, name, _)) in calls.iter().enumerate() {
            lines.push(cli_error(id, name, &format!("e{index}")));
        }
    }
    lines.push(json!({"type": "result", "subtype": "error_max_turns", "is_error": true, "session_id": "s", "usage": {}}));
    lines
}

struct Turn {
    exchange: Exchange,
    actions: Vec<Action>,
}

fn run(request: Request, map: &SessionMap, lines: &[Value]) -> Turn {
    let (mut exchange, mut actions) = Exchange::new(request, map, Config::new("/empty"), 0);
    for line in lines {
        actions.extend(exchange.on_stdout_line(&line.to_string(), 1));
    }
    actions.extend(exchange.on_stdout_eof(2));
    actions.extend(exchange.on_stderr_eof(2));
    actions.extend(exchange.on_exit(0, 3));
    Turn { exchange, actions }
}

fn effect(actions: &[Action]) -> MapEffect {
    actions
        .iter()
        .find_map(|a| match a {
            Action::Done(f) => Some(f.map_effect.clone()),
            _ => None,
        })
        .expect("Done")
}

/// Rebuilds each tool block the way an Anthropic SSE client does.
fn client_blocks_from_sse(actions: &[Action]) -> Vec<Value> {
    let mut blocks: Vec<(u64, Value, String)> = vec![];
    for action in actions {
        let Action::Emit(event) = action else {
            continue;
        };
        let data = &event.data;
        match data["type"].as_str() {
            Some("content_block_start") => blocks.push((
                data["index"].as_u64().unwrap(),
                data["content_block"].clone(),
                String::new(),
            )),
            Some("content_block_delta") if data["delta"]["type"] == "input_json_delta" => {
                let index = data["index"].as_u64().unwrap();
                let block = blocks.iter_mut().find(|b| b.0 == index).unwrap();
                block
                    .2
                    .push_str(data["delta"]["partial_json"].as_str().unwrap());
            }
            _ => {}
        }
    }
    blocks
        .into_iter()
        .map(|(_, mut block, json)| {
            block["input"] = serde_json::from_str(&json).unwrap();
            block
        })
        .collect()
}

fn json_response(actions: &[Action]) -> Value {
    actions
        .iter()
        .find_map(|a| match a {
            Action::RespondJson(200, body) => Some(body.clone()),
            _ => None,
        })
        .expect("200 JSON response")
}

fn raw(calls: &[(&str, &str, Value)]) -> Vec<String> {
    calls
        .iter()
        .map(|(_, _, input)| input.to_string())
        .collect()
}

fn assert_blocks(blocks: &[Value], calls: &[(&str, &str, Value)]) {
    assert_eq!(blocks.len(), calls.len());
    for (block, (id, name, input)) in blocks.iter().zip(calls) {
        assert_eq!(block["type"], "tool_use");
        assert_eq!(block["id"], *id);
        assert_eq!(block["name"], *name, "client sees the declared name");
        assert_eq!(
            &block["input"], input,
            "input forwarded with its JSON types"
        );
    }
}

#[test]
fn batch_preserves_typed_inputs_names_and_ids_streaming() {
    for shape in [Shape::Prefixed, Shape::BareInterleaved, Shape::BareTrailing] {
        let calls = calls();
        let turn = run(
            request_body(true, first_user()),
            &SessionMap::default(),
            &stdout(&calls, &raw(&calls), shape),
        );
        let blocks = client_blocks_from_sse(&turn.actions);
        assert_blocks(&blocks, &calls);
        assert!(blocks[0]["input"]["completed"].is_array());
        assert!(blocks[0]["input"]["remaining"].is_array());
        assert!(blocks[2]["input"]["files"].is_array());
        assert!(
            blocks[1]["input"]["new_string"].is_string(),
            "array-looking scalar stays a string"
        );
        let stop = turn
            .actions
            .iter()
            .rev()
            .find_map(|a| match a {
                Action::Emit(e) if e.event == "message_delta" => Some(e.data.clone()),
                _ => None,
            })
            .unwrap();
        assert_eq!(stop["delta"]["stop_reason"], "tool_use");
        // No CLI-generated tool error reaches the client.
        assert!(!turn.actions.iter().any(|a| matches!(a, Action::Emit(e)
            if e.data.to_string().contains("No such tool available"))));
    }
}

#[test]
fn batch_preserves_typed_inputs_names_and_ids_json() {
    for shape in [Shape::Prefixed, Shape::BareInterleaved, Shape::BareTrailing] {
        let calls = calls();
        let turn = run(
            request_body(false, first_user()),
            &SessionMap::default(),
            &stdout(&calls, &raw(&calls), shape),
        );
        let body = json_response(&turn.actions);
        assert_eq!(body["stop_reason"], "tool_use");
        assert_blocks(body["content"].as_array().unwrap(), &calls);
        assert!(!body.to_string().contains("No such tool available"));
    }
}

/// A model that emits an array argument as a string containing JSON array text
/// must reach the client unchanged: clod neither repairs nor rejects it.
#[test]
fn string_encoded_array_is_forwarded_unchanged() {
    let calls = vec![(
        "toolu_s",
        "progress_update",
        json!({"phase": "investigating", "completed": "[\"read exchange.rs\", \"ran tests\"]", "remaining": "[]"}),
    )];
    for stream in [true, false] {
        for shape in [Shape::Prefixed, Shape::BareInterleaved] {
            let turn = run(
                request_body(stream, first_user()),
                &SessionMap::default(),
                &stdout(&calls, &raw(&calls), shape),
            );
            let blocks = if stream {
                // The raw fragments are forwarded byte-for-byte.
                let sent: String = turn
                    .actions
                    .iter()
                    .filter_map(|a| match a {
                        Action::Emit(e) if e.data["delta"]["type"] == "input_json_delta" => Some(
                            e.data["delta"]["partial_json"]
                                .as_str()
                                .unwrap()
                                .to_string(),
                        ),
                        _ => None,
                    })
                    .collect();
                assert_eq!(sent, raw(&calls)[0]);
                client_blocks_from_sse(&turn.actions)
            } else {
                json_response(&turn.actions)["content"]
                    .as_array()
                    .unwrap()
                    .clone()
            };
            assert_blocks(&blocks, &calls);
            assert!(blocks[0]["input"]["completed"].is_string());
            assert_eq!(blocks[0]["input"]["remaining"], "[]");
        }
    }
}

/// The client's follow-up request: the assistant batch it received plus one
/// user message with a real result for every call.
fn follow_up(stream: bool, calls: &[(&str, &str, Value)]) -> Request {
    let tool_uses: Vec<Value> = calls
        .iter()
        .map(
            |(id, name, input)| json!({"type": "tool_use", "id": id, "name": name, "input": input}),
        )
        .collect();
    let results: Vec<Value> = calls
        .iter()
        .map(|(id, name, _)| json!({"type": "tool_result", "tool_use_id": id, "content": format!("REAL {name} {id}")}))
        .collect();
    let mut messages = first_user();
    let list = messages.as_array_mut().unwrap();
    list.push(json!({"role": "assistant", "content": tool_uses}));
    list.push(json!({"role": "user", "content": results}));
    request_body(stream, messages)
}

fn stdin_user(actions: &[Action]) -> Value {
    actions
        .iter()
        .filter_map(|a| match a {
            Action::WriteStdin(line) => serde_json::from_str::<Value>(line).ok(),
            _ => None,
        })
        .find(|v| v["type"] == "user")
        .expect("user stdin record")
}

#[test]
fn prefixed_batch_resumes_with_every_real_result() {
    let calls = calls();
    let turn = run(
        request_body(true, first_user()),
        &SessionMap::default(),
        &stdout(&calls, &raw(&calls), Shape::Prefixed),
    );
    let MapEffect::Commit(checkpoint) = effect(&turn.actions) else {
        panic!("prefixed batch should checkpoint")
    };
    assert_eq!(checkpoint.resume_uuid, "u2");
    assert_eq!(
        checkpoint.pending_tool_ids,
        vec!["toolu_a", "toolu_b", "toolu_c"]
    );
    let next = follow_up(true, &calls);
    let mut map = SessionMap::default();
    map.insert(session::key(&next).unwrap(), checkpoint);
    assert!(matches!(session::plan(&next, &map), Plan::Resume(_)));
    let (_, actions) = Exchange::new(next, &map, Config::new("/empty"), 10);
    let content = stdin_user(&actions)["message"]["content"].clone();
    let ids: Vec<&str> = content
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["tool_use_id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["toolu_a", "toolu_b", "toolu_c"]);
    for (block, (id, name, _)) in content.as_array().unwrap().iter().zip(&calls) {
        assert_eq!(block["content"], format!("REAL {name} {id}"));
    }
}

/// Regression for the observed failure: with interleaved CLI errors, the CLI
/// record to resume from descends from the first call's `No such tool`
/// result. That turn must not checkpoint; the next request replays the
/// client's history, which carries the real result for every call.
#[test]
fn interleaved_bare_batch_does_not_resume_past_cli_errors() {
    for stream in [true, false] {
        let calls = calls();
        let turn = run(
            request_body(stream, first_user()),
            &SessionMap::default(),
            &stdout(&calls, &raw(&calls), Shape::BareInterleaved),
        );
        let final_effect = effect(&turn.actions);
        assert_eq!(final_effect, MapEffect::Discard, "stream={stream}");
        assert_eq!(
            turn.exchange.checkpoint_reason(),
            Some("ToolResultBeforeLastToolCall")
        );

        let mut map = SessionMap::default();
        session::apply(
            &mut map,
            session::Final {
                key: session::key(&request_body(stream, first_user())),
                map_effect: final_effect,
            },
        );
        let next = follow_up(stream, &calls);
        assert!(matches!(session::plan(&next, &map), Plan::Fresh));
        let (_, actions) = Exchange::new(next, &map, Config::new("/empty"), 10);
        let replay = stdin_user(&actions).to_string();
        for (id, name, _) in &calls {
            assert!(replay.contains(&format!("REAL {name} {id}")), "{id}");
        }
        assert!(!replay.contains("No such tool available"));
    }
}

/// A single bare call's CLI error is a child of the resumed record (live
/// verified), and errors after the last tool record are likewise off the
/// resumed chain, so these still checkpoint and resume with real results.
#[test]
fn trailing_cli_errors_still_checkpoint() {
    for calls in [calls()[..1].to_vec(), calls()] {
        for shape in [Shape::BareInterleaved, Shape::BareTrailing] {
            if calls.len() > 1 && shape == Shape::BareInterleaved {
                continue;
            }
            let turn = run(
                request_body(true, first_user()),
                &SessionMap::default(),
                &stdout(&calls, &raw(&calls), shape),
            );
            let MapEffect::Commit(checkpoint) = effect(&turn.actions) else {
                panic!("{:?}", turn.exchange.checkpoint_reason())
            };
            let ids: Vec<&str> = calls.iter().map(|c| c.0).collect();
            assert_eq!(checkpoint.pending_tool_ids, ids);
            let next = follow_up(true, &calls);
            let mut map = SessionMap::default();
            map.insert(session::key(&next).unwrap(), checkpoint);
            let (_, actions) = Exchange::new(next, &map, Config::new("/empty"), 10);
            assert!(matches!(
                actions.first(),
                Some(Action::Spawn(spec)) if spec.args.iter().any(|a| a == "--resume")
            ));
            let content = stdin_user(&actions)["message"]["content"].to_string();
            for (id, name, _) in &calls {
                assert!(content.contains(&format!("REAL {name} {id}")));
            }
        }
    }
}
