use clod_core::{
    exchange::{Action, Exchange},
    protocol,
    request::{self, Request},
    session::{self, Checkpoint, MapEffect, SessionMap},
    spawn::{self, Config},
};
use serde_json::{json, Value};
fn request(stream: bool) -> Request {
    request::parse(&serde_json::to_vec(&json!({"model":"haiku","system":"test","messages":[{"role":"user","content":"hello"}],"tools":[],"max_tokens":2048,"stream":stream})).unwrap(), &[("x-polytoken-conversation","test")]).unwrap()
}
fn new(stream: bool, resume: bool) -> (Exchange, Vec<Action>) {
    let r = request(stream);
    let mut map = SessionMap::default();
    if resume {
        map.insert(
            session::key(&r).unwrap(),
            Checkpoint {
                session_id: "source".into(),
                resume_uuid: "old".into(),
                prefix_hash: session::prefix_hash(&[]),
                pending_tool_ids: vec![],
            },
        );
    }
    Exchange::new(r, &map, Config::new("/empty"), 0)
}
fn out(e: &mut Exchange, v: Value) -> Vec<Action> {
    e.on_stdout_line(&v.to_string(), 10)
}
fn event(e: &mut Exchange, v: Value) -> Vec<Action> {
    out(
        e,
        json!({"type":"stream_event","session_id":"fork","event":v}),
    )
}
fn begin(e: &mut Exchange) -> Vec<Action> {
    event(
        e,
        json!({"type":"message_start","message":{"id":"msg","type":"message","role":"assistant","content":[]}}),
    )
}
fn assistant(e: &mut Exchange, valid: bool) {
    out(
        e,
        json!({"type":"assistant","uuid":if valid{json!("new")}else{Value::Null},"session_id":"fork","message":{"content":[{"type":"text","text":"OK"}]}}),
    );
}
fn success(e: &mut Exchange) -> Vec<Action> {
    out(
        e,
        json!({"type":"result","subtype":"success","session_id":"fork","usage":{}}),
    )
}
fn reap(e: &mut Exchange) -> Vec<Action> {
    let mut a = e.on_stdout_eof(20);
    a.extend(e.on_stderr_eof(20));
    a.extend(e.on_exit(0, 20));
    a
}
fn effect(a: &[Action]) -> &MapEffect {
    a.iter()
        .find_map(|a| {
            if let Action::Done(f) = a {
                Some(&f.map_effect)
            } else {
                None
            }
        })
        .expect("Done")
}
fn status(a: &[Action], status: u16) -> bool {
    a.iter()
        .any(|a| matches!(a,Action::RespondJson(s,_) if *s==status))
}
fn emits(a: &[Action], name: &str) -> usize {
    a.iter()
        .filter(|a| matches!(a,Action::Emit(e) if e.event==name))
        .count()
}

/// Captures group stdin before stdout, so compare the generated writes after
/// driving stdout. JSON subset matching permits capture-only schema metadata;
/// only the locally allocated initialization id and appended note are normalized.
fn subset(expected: &Value, actual: &Value) -> bool {
    match (expected, actual) {
        (Value::Object(e), Value::Object(a)) => e
            .iter()
            .all(|(k, v)| a.get(k).is_some_and(|a| subset(v, a))),
        (Value::Array(e), Value::Array(a)) => {
            e.len() == a.len() && e.iter().zip(a).all(|(e, a)| subset(e, a))
        }
        _ => expected == actual,
    }
}
fn replay_fixture(
    fixture: &str,
    exchange: &mut Exchange,
    initial: Vec<Action>,
    clock: &mut impl Iterator<Item = u64>,
) -> Vec<Action> {
    let rows = fixture
        .lines()
        .map(|s| serde_json::from_str::<Value>(s).unwrap())
        .collect::<Vec<_>>();
    let mut actions = initial;
    for row in &rows {
        let now = clock.next().expect("scripted clock");
        match row["dir"].as_str() {
            Some("out") => actions.extend(exchange.on_stdout_line(&row["line"].to_string(), now)),
            Some("err") => {
                actions.extend(exchange.on_stderr_line(row["line"].as_str().unwrap_or(""), now))
            }
            _ => (),
        }
    }
    let expected = rows
        .iter()
        .filter(|r| r["dir"] == "in")
        .map(|r| r["line"].clone())
        .collect::<Vec<_>>();
    let actual = actions
        .iter()
        .filter_map(|a| {
            if let Action::WriteStdin(s) = a {
                Some(serde_json::from_str::<Value>(s).unwrap())
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(expected.len(), actual.len(), "stdin count");
    for (mut e, mut a) in expected.into_iter().zip(actual) {
        if e["request"]["subtype"] == "initialize" {
            e["request_id"] = a["request_id"].clone();
            let p = a["request"]["systemPrompt"][0].as_str().unwrap();
            a["request"]["systemPrompt"][0] = json!(p
                .strip_suffix(clod_core::replay::PROVENANCE_NOTE)
                .unwrap_or(p));
        }
        if e["type"] == "user"
            && e["message"]["content"].is_string()
            && a["message"]["content"].is_array()
        {
            let blocks = a["message"]["content"].as_array().unwrap();
            if blocks.len() == 1 && blocks[0]["type"] == "text" {
                a["message"]["content"] = blocks[0]["text"].clone();
            }
        }
        assert!(subset(&e, &a), "stdin mismatch\nexpected {e}\nactual {a}");
    }
    actions
}
fn captured(name: &str, stream: bool) -> (Exchange, Vec<Action>) {
    let text = std::fs::read_to_string(format!(
        "{}/tests/fixtures/{name}.ndjson",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    let rows = text
        .lines()
        .map(|s| serde_json::from_str::<Value>(s).unwrap())
        .collect::<Vec<_>>();
    let init = &rows
        .iter()
        .find(|r| r["dir"] == "in" && r["line"]["request"]["subtype"] == "initialize")
        .unwrap()["line"];
    let user = &rows
        .iter()
        .find(|r| r["dir"] == "in" && r["line"]["type"] == "user")
        .unwrap()["line"];
    let tools = rows
        .iter()
        .find_map(|r| {
            r["line"]["response"]["response"]["mcp_response"]["result"]["tools"].as_array()
        })
        .map(|tools| {
            tools
                .iter()
                .map(|t| {
                    let mut schema = t["inputSchema"].clone();
                    schema.as_object_mut().unwrap().remove("$schema");
                    json!({"name":t["name"],"description":t["description"],"input_schema":schema})
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let r=request::parse(&serde_json::to_vec(&json!({"model":"haiku","system":init["request"]["systemPrompt"][0],"messages":[user["message"].clone()],"tools":tools,"max_tokens":2048,"stream":stream})).unwrap(),&[("x-polytoken-session","capture")]).unwrap();
    let evidence: Value =
        serde_json::from_str(include_str!("fixtures/capture-evidence.json")).unwrap();
    let args = evidence
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["name"] == name)
        .unwrap()["argv"]
        .as_array()
        .unwrap();
    let flag = |name: &str| {
        args.iter()
            .position(|v| v == name)
            .map(|i| args[i + 1].as_str().unwrap().to_string())
    };
    let mut map = SessionMap::default();
    if let Some(session_id) = flag("--resume") {
        let pending_tool_ids = r.messages[0]
            .content
            .blocks()
            .iter()
            .filter_map(|b| b["tool_use_id"].as_str().map(String::from))
            .collect();
        map.insert(
            session::key(&r).unwrap(),
            Checkpoint {
                session_id,
                resume_uuid: flag("--resume-session-at").unwrap(),
                pending_tool_ids,
                prefix_hash: session::prefix_hash(&[]),
            },
        );
    }
    let (mut e, a) = Exchange::new(r, &map, Config::new("/empty"), 0);
    let a = replay_fixture(&text, &mut e, a, &mut (0..).map(|n| n * 10));
    (e, a)
}
#[test]
fn fixtures_parse() {
    for name in [
        "a-text",
        "a-text-resume",
        "b-tool",
        "b-tool-resume",
        "c-parallel",
        "c-second-round",
        "c-final",
        "d-thinking",
        "e-image",
        "f-missing-session",
        "f-missing-uuid",
    ] {
        let (mut e, actions) = captured(name, true);
        let a = reap(&mut e);
        if name == "e-image" {
            // The capture contains an explicit media_removed API error before
            // a later success; it must not checkpoint that failed exchange.
            assert_eq!(effect(&a), &MapEffect::Keep);
            assert!(status(&actions, 500));
        } else if !name.starts_with('f') {
            assert!(matches!(effect(&a), MapEffect::Commit(_)), "{name}");
        }
    }
}
#[test]
#[should_panic(expected = "stdin mismatch")]
fn replayer_mismatch_fails() {
    let (mut e, a) = new(true, false);
    replay_fixture(
        "{\"dir\":\"in\",\"line\":{\"wrong\":true}}\n{\"dir\":\"in\",\"line\":{}}",
        &mut e,
        a,
        &mut (0..),
    );
}
#[test]
fn text_stream() {
    let (mut e, a) = captured("a-text", true);
    assert_eq!(
        a.iter().filter(|a| matches!(a, Action::StartSse)).count(),
        1
    );
    assert_eq!(emits(&a, "message_delta"), 1);
    assert_eq!(emits(&a, "message_stop"), 1);
    assert!(matches!(effect(&reap(&mut e)), MapEffect::Commit(_)));
}
#[test]
fn text_nonstream_no_sse_actions() {
    let (_, a) = captured("a-text", false);
    assert!(!a
        .iter()
        .any(|a| matches!(a, Action::Emit(_) | Action::StartSse)));
    assert!(status(&a, 200));
}
#[test]
fn terminal_only_after_result() {
    let (mut e, _) = new(true, false);
    begin(&mut e);
    assert!(event(
        &mut e,
        json!({"type":"message_delta","delta":{"stop_reason":"end_turn"}})
    )
    .is_empty());
    assert!(event(&mut e, json!({"type":"message_stop"})).is_empty());
    assert_eq!(emits(&success(&mut e), "message_stop"), 1);
}
#[test]
fn done_after_eof_and_exit() {
    let (mut e, _) = new(true, false);
    assistant(&mut e, true);
    success(&mut e);
    assert!(e.on_exit(0, 20).is_empty());
    assert!(e.on_stdout_eof(20).is_empty());
    assert!(matches!(effect(&e.on_stderr_eof(20)), MapEffect::Commit(_)));
}
#[test]
fn tool_single() {
    let (mut e, a) = captured("b-tool", true);
    assert!(a
        .iter()
        .any(|a| matches!(a,Action::Emit(ev) if ev.data["content_block"]["name"]=="lookup")));
    assert!(matches!(effect(&reap(&mut e)),MapEffect::Commit(cp) if cp.pending_tool_ids.len()==1));
}
#[test]
fn tool_parallel_multi_record() {
    let (mut e, a) = captured("c-parallel", true);
    assert_eq!(
        a.iter()
            .filter(|a| matches!(a,Action::WriteStdin(s) if s.contains("permissionDecision")))
            .count(),
        3
    );
    assert!(matches!(effect(&reap(&mut e)),MapEffect::Commit(cp) if cp.pending_tool_ids.len()==3));
}
#[test]
fn checkpoint_uses_forked_session_id() {
    let (mut e, _) = captured("c-final", true);
    assert!(
        matches!(effect(&reap(&mut e)),MapEffect::Commit(cp) if cp.session_id=="36ede346-28b4-4a6b-96e0-271c2d42b4dc")
    );
}
#[test]
fn max_turns_without_tools_is_error() {
    let (mut e, _) = new(true, false);
    assert!(status(
        &out(
            &mut e,
            json!({"type":"result","subtype":"error_max_turns","errors":["Reached maximum number of turns (1)"]})
        ),
        500
    ));
}
#[test]
fn hook_after_generation_not_held() {
    let (mut e, _) = new(true, false);
    begin(&mut e);
    event(&mut e, json!({"type":"message_delta"}));
    assert!(
        matches!(&out(&mut e,json!({"type":"control_request","request_id":"hook","request":{"subtype":"hook_callback"}}))[0],Action::WriteStdin(s) if s.contains("deny"))
    );
}
#[test]
fn hook_hold_backstop_discards() {
    let (mut e, _) = new(true, false);
    begin(&mut e);
    assert!(out(
        &mut e,
        json!({"type":"control_request","request_id":"hook","request":{"subtype":"hook_callback"}})
    )
    .is_empty());
    assert!(e
        .on_tick(60_010)
        .iter()
        .any(|a| matches!(a, Action::WriteStdin(_))));
    assistant(&mut e, true);
    success(&mut e);
    assert_eq!(effect(&reap(&mut e)), &MapEffect::Discard);
}
#[test]
fn tools_call_returns_mcp_error() {
    let (mut e, _) = new(true, false);
    let a = out(
        &mut e,
        json!({"type":"control_request","request_id":"mcp","request":{"subtype":"mcp_message","message":{"method":"tools/call","id":7}}}),
    );
    assert!(matches!(&a[0],Action::WriteStdin(s) if s.contains("isError")));
}
fn stale(e: &mut Exchange) -> Vec<Action> {
    out(
        e,
        json!({"type":"result","subtype":"error_during_execution","errors":["No conversation found with session ID: source"]}),
    )
}
#[test]
fn stale_prestream_respawns_fresh() {
    let (mut e, _) = new(true, true);
    assert_eq!(stale(&mut e), vec![Action::CloseStdin]);
    let a = reap(&mut e);
    assert!(matches!(a.first(),Some(Action::Spawn(s)) if !s.args.contains(&"--resume".into())));
}
#[test]
fn stale_on_stderr_only_respawns_fresh() {
    let (mut e, _) = new(true, true);
    e.on_stderr_line("No message found with message.uuid of: old", 10);
    assert!(matches!(reap(&mut e).first(), Some(Action::Spawn(_))));
}
#[test]
fn stale_stderr_after_stdout_eof_retries() {
    let (mut e, _) = new(true, true);
    e.on_stdout_eof(10);
    e.on_exit(1, 11);
    e.on_stderr_line("No conversation found with session ID: source", 12);
    assert!(matches!(
        e.on_stderr_eof(13).first(),
        Some(Action::Spawn(_))
    ));
}
#[test]
fn stale_retried_once_only() {
    let (mut e, _) = new(true, true);
    stale(&mut e);
    reap(&mut e);
    stale(&mut e);
    assert_eq!(effect(&reap(&mut e)), &MapEffect::Discard);
}
#[test]
fn stale_then_fresh_failure_discards() {
    let (mut e, _) = new(true, true);
    stale(&mut e);
    reap(&mut e);
    assert_eq!(effect(&e.on_spawn_failed("bad", 30)), &MapEffect::Discard);
}
#[test]
fn stale_after_commit_sse_error_discard() {
    let (mut e, _) = new(true, true);
    begin(&mut e);
    assert_eq!(emits(&stale(&mut e), "error"), 1);
    assert_eq!(effect(&reap(&mut e)), &MapEffect::Discard);
}
#[test]
fn resume_crash_keeps() {
    let (mut e, _) = new(true, true);
    begin(&mut e);
    assert_eq!(effect(&reap(&mut e)), &MapEffect::Keep);
}
#[test]
fn resume_partial_error_keeps() {
    let (mut e, _) = new(true, true);
    begin(&mut e);
    out(
        &mut e,
        json!({"type":"result","subtype":"error_during_execution","errors":["bad"]}),
    );
    assert_eq!(effect(&reap(&mut e)), &MapEffect::Keep);
}
#[test]
fn resume_disconnect_keeps() {
    let (mut e, _) = new(true, true);
    e.on_client_gone(1);
    assert_eq!(effect(&reap(&mut e)), &MapEffect::Keep);
}
#[test]
fn exit_without_result_or_events_on_resume_discards() {
    let (mut e, _) = new(true, true);
    assert_eq!(effect(&reap(&mut e)), &MapEffect::Discard);
}
#[test]
fn disconnect_after_result_keeps_selected_effect() {
    for valid in [true, false] {
        let (mut e, _) = new(true, true);
        assistant(&mut e, valid);
        success(&mut e);
        e.on_client_gone(11);
        let a = reap(&mut e);
        assert_eq!(matches!(effect(&a), MapEffect::Commit(_)), valid);
        if !valid {
            assert_eq!(effect(&a), &MapEffect::Discard);
        }
    }
}
#[test]
fn error_result_closes_stdin_and_reaps() {
    let (mut e, _) = new(true, false);
    assert!(out(&mut e, json!({"type":"result","subtype":"bad"})).contains(&Action::CloseStdin));
    assert_eq!(effect(&reap(&mut e)), &MapEffect::Keep);
}
#[test]
fn interrupt_write_failure_accelerates_kill() {
    let (mut e, _) = new(true, true);
    e.on_client_gone(1);
    assert_eq!(e.on_stdin_failed("broken", 2), vec![Action::Kill]);
    assert_eq!(effect(&reap(&mut e)), &MapEffect::Keep);
    assert!(e.on_stdin_failed("again", 3).is_empty());
}
#[test]
fn exactly_one_done_all_paths() {
    for path in [
        "success",
        "invalid",
        "error",
        "stale",
        "spawn",
        "stdin",
        "disconnect",
        "timeout",
        "exit",
        "partial",
    ] {
        for resume in [false, true] {
            let (mut e, mut a) = new(true, resume);
            match path {
                "success" | "invalid" => {
                    assistant(&mut e, path == "success");
                    a.extend(success(&mut e));
                }
                "error" => a.extend(out(&mut e, json!({"type":"result","subtype":"bad"}))),
                "stale" => a.extend(stale(&mut e)),
                "spawn" => a.extend(e.on_spawn_failed("bad", 1)),
                "stdin" => a.extend(e.on_stdin_failed("bad", 1)),
                "disconnect" => a.extend(e.on_client_gone(1)),
                "timeout" => a.extend(e.on_tick(900_000)),
                "partial" => a.extend(begin(&mut e)),
                _ => (),
            };
            a.extend(reap(&mut e));
            if a.iter().filter(|a| matches!(a, Action::Spawn(_))).count() == 2 {
                a.extend(e.on_spawn_failed("retry failed", 30));
            }
            a.extend(e.on_tick(999_999));
            a.extend(reap(&mut e));
            a.extend(e.on_client_gone(999_999));
            assert_eq!(
                a.iter().filter(|a| matches!(a, Action::Done(_))).count(),
                1,
                "{path} resume={resume}"
            );
            if resume && matches!(path, "disconnect" | "timeout") {
                assert_eq!(effect(&a), &MapEffect::Keep);
            }
        }
    }
}
#[test]
fn nonstream_error_after_internal_content() {
    let (mut e, _) = new(false, false);
    begin(&mut e);
    assert!(status(
        &out(&mut e, json!({"type":"result","subtype":"bad"})),
        500
    ));
}
#[test]
fn error_usage_limit_precommit() {
    let (mut e, _) = new(true, false);
    assert!(status(
        &out(
            &mut e,
            json!({"type":"result","subtype":"bad","errors":["rate limit exceeded"]})
        ),
        429
    ));
}
#[test]
fn error_usage_limit_postcommit() {
    let (mut e, _) = new(true, false);
    begin(&mut e);
    assert_eq!(
        emits(
            &out(
                &mut e,
                json!({"type":"result","subtype":"bad","errors":["rate limit exceeded"]})
            ),
            "error"
        ),
        1
    );
}
#[test]
fn error_after_content_no_stop() {
    let (mut e, _) = new(true, false);
    begin(&mut e);
    let a = out(
        &mut e,
        json!({"type":"result","subtype":"bad","errors":["bad"]}),
    );
    assert_eq!(emits(&a, "error"), 1);
    assert_eq!(emits(&a, "message_stop"), 0);
}
#[test]
fn error_context_overflow() {
    let (mut e, _) = new(true, false);
    assert!(status(
        &out(
            &mut e,
            json!({"type":"result","subtype":"bad","errors":["Prompt is too long"]})
        ),
        400
    ));
}
#[test]
fn exit_without_result_500() {
    let (mut e, _) = new(false, false);
    assert!(status(&reap(&mut e), 500));
}
#[test]
fn spawn_failed_500() {
    let (mut e, _) = new(false, false);
    assert!(status(&e.on_spawn_failed("bad", 1), 500));
}
#[test]
fn stdin_failed_500() {
    let (mut e, _) = new(false, false);
    assert!(status(&e.on_stdin_failed("bad", 1), 500));
}
#[test]
fn tool_name_too_long_no_spawn() {
    let mut r = request(true);
    r.tools = vec![json!({"name":"x".repeat(56)})];
    let (_, a) = Exchange::new(r, &SessionMap::default(), Config::new("/empty"), 0);
    assert!(status(&a, 400));
    assert!(!a.iter().any(|a| matches!(a, Action::Spawn(_))));
}
#[test]
fn ping_interval_streaming_only() {
    for stream in [true, false] {
        let (mut e, _) = new(stream, false);
        begin(&mut e);
        assert_eq!(emits(&e.on_tick(15_009), "ping"), 0);
        assert_eq!(emits(&e.on_tick(15_010), "ping"), usize::from(stream));
    }
}
#[test]
fn disconnect_interrupt_then_kill() {
    let (mut e, _) = new(true, false);
    assert!(matches!(&e.on_client_gone(10)[0],Action::WriteStdin(s) if s.contains("interrupt")));
    assert!(e.on_tick(2009).is_empty());
    assert_eq!(e.on_tick(2010), vec![Action::Kill]);
}
#[test]
fn close_stdin_then_kill_keeps_commit() {
    let (mut e, _) = new(true, false);
    assistant(&mut e, true);
    success(&mut e);
    assert_eq!(e.on_tick(5010), vec![Action::Kill]);
    assert!(matches!(effect(&reap(&mut e)), MapEffect::Commit(_)));
}
#[test]
fn turn_timeout_keeps() {
    let (mut e, _) = new(true, true);
    assert!(status(&e.on_tick(900_000), 500));
    assert_eq!(e.on_tick(902_000), vec![Action::Kill]);
    assert_eq!(effect(&reap(&mut e)), &MapEffect::Keep);
}
#[test]
fn spawn_spec_resume_flags() {
    let (_, a) = new(true, true);
    assert!(
        matches!(&a[0],Action::Spawn(s) if s.args.windows(2).any(|w|w==["--resume-session-at","old"]) && s.args.contains(&"--fork-session".into()))
    );
}
#[test]
fn resume_stdin_single_message() {
    let (_, a) = new(true, true);
    assert!(
        matches!(&a[2],Action::WriteStdin(s) if !s.contains("conversation_history") && serde_json::from_str::<Value>(s).unwrap()["message"]["content"]=="hello")
    );
}
#[test]
fn spawn_spec_env_and_flags() {
    let (_, a) = new(true, false);
    let Action::Spawn(s) = &a[0] else { panic!() };
    assert_eq!(s.working_directory, "/empty");
    assert!(s.args.contains(&"--tools".into()));
    assert!(s.args.contains(&"--setting-sources=".into()));
    assert!(s.args.contains(&"--strict-mcp-config".into()));
    assert!(s
        .env_set
        .contains(&("ENABLE_TOOL_SEARCH".into(), "false".into())));
    assert!(s.env_remove.contains(&"CLAUDE_CONFIG_DIR".into()));
}
#[test]
fn spawn_spec_upstream_override() {
    let r = request(false);
    let mut config = Config::new("/empty");
    let normal = spawn::spec(&r, &session::Plan::Fresh, &config);
    assert!(normal.env_remove.contains(&"ANTHROPIC_BASE_URL".into()));
    config.upstream_base_url = Some("http://127.0.0.1:19876".into());
    let captured = spawn::spec(&r, &session::Plan::Fresh, &config);
    assert!(!captured.env_remove.contains(&"ANTHROPIC_BASE_URL".into()));
    assert!(captured
        .env_set
        .contains(&("ANTHROPIC_BASE_URL".into(), "http://127.0.0.1:19876".into())));
}
#[test]
fn spawn_spec_system_prompt() {
    for resume in [true, false] {
        let (_, a) = new(true, resume);
        let Action::WriteStdin(s) = &a[1] else {
            panic!()
        };
        let v: Value = serde_json::from_str(s).unwrap();
        assert!(v["request"]["systemPrompt"].is_array());
        assert_eq!(s.contains("clod-note"), !resume);
        assert!(!s.contains("meridian-note"));
    }
}
#[test]
fn thinking_display_default() {
    let mut r = request(true);
    r.thinking = Some(json!({"type":"enabled","budget_tokens":1024}));
    r.output_config = Some(json!({"effort":"high"}));
    let s = spawn::spec(&r, &session::Plan::Fresh, &Config::new("/empty"));
    assert!(s
        .args
        .windows(2)
        .any(|w| w == ["--thinking-display", "summarized"]));
    assert!(s.args.windows(2).any(|w| w == ["--effort", "high"]));
}
#[test]
fn thinking_signature_forwarding_meridian_synthetic() {
    let (mut e, _) = new(true, false);
    for line in include_str!("fixtures/thinking-meridian-synthetic.ndjson").lines() {
        let row: Value = serde_json::from_str(line).unwrap();
        assert_eq!(row["synthetic"], true);
        let actions = out(&mut e, row["line"].clone());
        let forwarded = actions
            .iter()
            .find_map(|a| {
                if let Action::Emit(e) = a {
                    Some(&e.data)
                } else {
                    None
                }
            })
            .unwrap();
        assert_eq!(forwarded, &row["line"]["event"]);
    }
}
#[test]
fn stdin_failure_exit_effect_depends_on_who_killed() {
    for kill in [false, true] {
        let (mut e, _) = new(true, true);
        e.on_stdin_failed("broken", 1);
        if kill {
            e.on_tick(5001);
        }
        assert_eq!(
            effect(&reap(&mut e)),
            if kill {
                &MapEffect::Keep
            } else {
                &MapEffect::Discard
            }
        );
    }
}
#[test]
fn duplicate_stale_stderr_emits_one_error() {
    let (mut e, _) = new(true, true);
    begin(&mut e);
    assert_eq!(
        emits(
            &e.on_stderr_line("No conversation found with session ID: x", 11),
            "error"
        ),
        1
    );
    assert!(e
        .on_stderr_line("No conversation found with session ID: x", 12)
        .is_empty());
}
#[test]
fn captured_tool_input_deltas_identical_and_checkpoint_resumes() {
    let (mut e, actions) = captured("b-tool", true);
    let fixture = include_str!("fixtures/b-tool.ndjson");
    let expected = fixture
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .filter(|r| r["dir"] == "out" && r["line"]["event"]["delta"]["type"] == "input_json_delta")
        .map(|r| r["line"]["event"].clone())
        .collect::<Vec<_>>();
    let actual = actions
        .iter()
        .filter_map(|a| match a {
            Action::Emit(ev) if ev.data["delta"]["type"] == "input_json_delta" => {
                Some(ev.data.clone())
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(expected, actual);
    let a = reap(&mut e);
    let MapEffect::Commit(cp) = effect(&a) else {
        panic!()
    };
    let mut r = request(true);
    r.messages = e_request_messages_from_fixture();
    let content = actions
        .iter()
        .filter_map(|a| {
            if let Action::Emit(ev) = a {
                Some(&ev.data)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    let start = content
        .iter()
        .find(|v| v["content_block"]["type"] == "tool_use")
        .unwrap();
    let fragments = expected
        .iter()
        .map(|v| v["delta"]["partial_json"].as_str().unwrap())
        .collect::<String>();
    let mut block = start["content_block"].clone();
    block["input"] = serde_json::from_str(&fragments).unwrap();
    r.messages
        .push(serde_json::from_value(json!({"role":"assistant","content":[block]})).unwrap());
    r.messages.push(serde_json::from_value(json!({"role":"user","content":[{"type":"tool_result","tool_use_id":cp.pending_tool_ids[0],"content":"value"}]})).unwrap());
    let mut map = SessionMap::default();
    map.insert(session::key(&r).unwrap(), cp.clone());
    assert!(matches!(session::plan(&r, &map), session::Plan::Resume(_)));
}
#[test]
fn normalized_assistant_input_does_not_replace_streamed_input() {
    for stream in [false, true] {
        for with_events in [false, true] {
            let mut r = request(stream);
            r.tools = vec![
                json!({"name":"collect","input_schema":{"type":"object","properties":{"items":{"type":"array","items":{"type":"string"}}}}}),
            ];
            let (mut e, _) =
                Exchange::new(r.clone(), &SessionMap::default(), Config::new("/empty"), 0);
            let raw = json!({"items":"[\"a\"]"});
            let normalized = json!({"items":["a"]});
            let mut actions = vec![];
            if with_events {
                actions.extend(begin(&mut e));
                actions.extend(event(&mut e, json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"call","name":"mcp__oc__collect","input":{}}})));
                let delta = json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":raw.to_string()}});
                let emitted = event(&mut e, delta.clone());
                if stream {
                    assert!(emitted
                        .iter()
                        .any(|a| matches!(a, Action::Emit(ev) if ev.data == delta)));
                } else {
                    assert!(!emitted.iter().any(|a| matches!(a, Action::Emit(_))));
                }
                actions.extend(emitted);
            }
            actions.extend(out(&mut e, json!({"type":"assistant","uuid":"normalized","session_id":"fork","message":{"content":[{"type":"tool_use","id":"call","name":"mcp__oc__collect","input":normalized}]}})));
            if with_events {
                actions.extend(event(
                    &mut e,
                    json!({"type":"content_block_stop","index":0}),
                ));
                actions.extend(event(
                    &mut e,
                    json!({"type":"message_delta","delta":{"stop_reason":"tool_use"}}),
                ));
            }
            actions.extend(success(&mut e));
            actions.extend(reap(&mut e));
            let MapEffect::Commit(cp) = effect(&actions) else {
                panic!("expected checkpoint: stream={stream}, with_events={with_events}")
            };
            let expected = if with_events { &raw } else { &normalized };
            if !stream {
                let body = actions
                    .iter()
                    .find_map(|a| match a {
                        Action::RespondJson(200, body) => Some(body),
                        _ => None,
                    })
                    .unwrap();
                assert_eq!(&body["content"][0]["input"], expected);
            }
            r.messages.push(serde_json::from_value(json!({"role":"assistant","content":[{"type":"tool_use","id":"call","name":"collect","input":expected}]})).unwrap());
            r.messages.push(serde_json::from_value(json!({"role":"user","content":[{"type":"tool_result","tool_use_id":"call","content":"ok"}]})).unwrap());
            let mut map = SessionMap::default();
            map.insert(session::key(&r).unwrap(), cp.clone());
            assert!(matches!(session::plan(&r, &map), session::Plan::Resume(_)));
            let opposite = if with_events { &normalized } else { &raw };
            let mut altered = r.clone();
            altered.messages[1] = serde_json::from_value(json!({"role":"assistant","content":[{"type":"tool_use","id":"call","name":"collect","input":opposite}]})).unwrap();
            assert!(matches!(
                session::fresh_reason(&altered, &map, session::KeyMode::Strict),
                Some(session::FreshReason::PrefixMismatch { .. })
            ));
        }
    }
}

fn e_request_messages_from_fixture() -> Vec<clod_core::request::Message> {
    include_str!("fixtures/b-tool.ndjson")
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .find(|r| r["dir"] == "in" && r["line"]["type"] == "user")
        .map(|r| vec![serde_json::from_value(r["line"]["message"].clone()).unwrap()])
        .unwrap()
}
#[test]
fn replay_golden_provenance() {
    let expected: String =
        serde_json::from_str(include_str!("golden/provenance.expected.json")).unwrap();
    assert_eq!(clod_core::replay::PROVENANCE_NOTE, expected);
}
#[test]
fn auth_error_401() {
    let (mut e, _) = new(false, false);
    assert!(status(
        &out(
            &mut e,
            json!({"type":"result","subtype":"error_during_execution","errors":["API Error: 401 authentication_error"]})
        ),
        401
    ));
}
#[test]
fn unknown_protocol_record_retained() {
    assert!(
        matches!(protocol::parse("{\"type\":\"future\",\"value\":1}").unwrap(),protocol::Record::Unknown(v) if v["value"]==1)
    );
}
