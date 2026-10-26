//! Constructive lifecycle histories; checkpoint expectations come from client history.
use super::*;
use clod_core::request::Request;
use serde_json::Value;
use sha2::{Digest, Sha256};

#[derive(Clone, Debug)]
struct ToolHistory {
    stream: bool,
    resume: bool,
    // false = prefixed, true = declared bare name.
    bare: Vec<bool>,
    payload: String,
    // 0 valid, 1 missing UUID, 2 recorded ID mismatch, 3 hold idle backstop,
    // 4 trailing text, 5 malformed streamed input.
    invalid: u8,
    max_turns: bool,
    after: Vec<u8>,
}
fn tool_histories() -> impl Strategy<Value = ToolHistory> {
    (
        any::<bool>(),
        any::<bool>(),
        prop::collection::vec(any::<bool>(), 1..5),
        ".{0,20}",
        0u8..6,
        any::<bool>(),
        prop::collection::vec(0u8..6, 0..15),
    )
        .prop_map(
            |(stream, resume, bare, payload, invalid, max_turns, after)| ToolHistory {
                stream,
                resume,
                bare,
                payload,
                invalid,
                max_turns,
                after,
            },
        )
}
fn req(stream: bool) -> Request {
    request::parse(&serde_json::to_vec(&json!({"model":"haiku","system":"test","messages":[{"role":"user","content":"hello"}],"tools":[{"name":"lookup","description":"test","input_schema":{"type":"object"}}],"max_tokens":2048,"stream":stream})).unwrap(),&[("x-polytoken-conversation","property")]).unwrap()
}
fn send(e: &mut Exchange, v: Value, now: u64) -> Vec<Action> {
    e.on_stdout_line(&v.to_string(), now)
}
fn ev(v: Value) -> Value {
    json!({"type":"stream_event","session_id":"fork-two","event":v})
}
fn hook(id: usize) -> Value {
    json!({"type":"control_request","request_id":format!("held-{id}"),"request":{"subtype":"hook_callback","callback_id":"hook_0","input":{"tool_name":"mcp__oc__lookup"}}})
}
fn denies(a: &[Action]) -> Vec<String> {
    a.iter()
        .filter_map(|a| {
            if let Action::WriteStdin(s) = a {
                let v: Value = serde_json::from_str(s).unwrap();
                if v["response"]["response"]["hookSpecificOutput"]["permissionDecision"] == "deny" {
                    return v["response"]["request_id"].as_str().map(str::to_owned);
                }
                None
            } else {
                None
            }
        })
        .collect()
}
fn terminal(e: &mut Exchange, now: u64) -> Vec<Action> {
    let mut a = e.on_stdout_eof(now);
    a.extend(e.on_stderr_eof(now));
    a.extend(e.on_exit(0, now));
    a
}
fn final_effect(a: &[Action]) -> MapEffect {
    let done = a
        .iter()
        .filter_map(|a| {
            if let Action::Done(f) = a {
                Some(f.map_effect.clone())
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(done.len(), 1, "terminal actions: {a:?}");
    done[0].clone()
}
static TOOL_COUNTS: [AtomicUsize; 5] = [const { AtomicUsize::new(0) }; 5]; // prefixed, bare, mixed, invalid, multi-record Commit
fn check_tool(t: &ToolHistory) {
    let (mut e, mut actions) = setup(t.stream, t.resume);
    let mut now = 100;
    actions.extend(send(&mut e,ev(json!({"type":"message_start","message":{"id":"tool-message","type":"message","role":"assistant","content":[]}})),now));
    let mut expected = vec![];
    for (i, bare) in t.bare.iter().enumerate() {
        let id = format!("call-{i}");
        let input = json!({"key":format!("{}-{i}",t.payload)});
        let name = if *bare { "lookup" } else { "mcp__oc__lookup" };
        actions.extend(send(&mut e,ev(json!({"type":"content_block_start","index":i,"content_block":{"type":"tool_use","id":id,"name":name,"input":{}}})),now));
        let mut raw = input.to_string();
        if t.invalid == 5 {
            raw.pop();
        }
        actions.extend(send(&mut e,ev(json!({"type":"content_block_delta","index":i,"delta":{"type":"input_json_delta","partial_json":raw}})),now));
        actions.extend(send(
            &mut e,
            ev(json!({"type":"content_block_stop","index":i})),
            now,
        ));
        let reply = send(&mut e, hook(i), now);
        assert!(denies(&reply).is_empty(), "hook released during generation");
        actions.extend(reply);
        let block = json!({"type":"tool_use","id":id,"name":"lookup","input":input});
        expected.push(block.clone());
        let mut recorded = block;
        recorded["name"] = json!(name);
        if t.invalid == 2 {
            recorded["id"] = json!(format!("wrong-{i}"));
        }
        actions.extend(send(&mut e,json!({"type":"assistant","session_id":"fork-two","uuid":if t.invalid==1&&i==t.bare.len()-1{Value::Null}else{json!(format!("record-{i}"))},"message":{"content":[recorded]}}),now));
    }
    if t.invalid == 3 {
        now += 60000;
        let a = e.on_tick(now);
        assert_eq!(denies(&a).len(), t.bare.len());
        actions.extend(a);
    }
    let boundary = send(
        &mut e,
        ev(
            json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":2}}),
        ),
        now,
    );
    assert_eq!(
        denies(&boundary).len(),
        if t.invalid == 3 { 0 } else { t.bare.len() }
    );
    actions.extend(boundary);
    actions.extend(send(&mut e, ev(json!({"type":"message_stop"})), now));
    let a = send(&mut e, hook(100), now);
    assert_eq!(denies(&a), vec!["held-100"]);
    actions.extend(a);
    if t.invalid == 4 {
        actions.extend(send(&mut e,json!({"type":"assistant","uuid":"trailing","session_id":"fork-two","message":{"content":[{"type":"text","text":"later"}]}}),now));
    }
    let result = send(
        &mut e,
        json!({"type":"result","subtype":if t.max_turns{"error_max_turns"}else{"success"},"session_id":"fork-two","errors":["Reached maximum number of turns (1)"],"usage":{}}),
        now,
    );
    assert!(
        result.iter().any(|a| match a {
            Action::RespondJson(s, _) => *s == 200,
            Action::Emit(v) => v.event == "message_stop",
            _ => false,
        }),
        "tool result not successful: {result:?}"
    );
    actions.extend(result);
    // These observations occur only after result classification. No later evidence
    // may replace the checkpoint (including stale stderr, disconnect or timeout).
    for op in &t.after {
        let a = match op {
            0 => e.on_client_gone(now),
            1 => {
                now += 900000;
                e.on_tick(now)
            }
            2 => e.on_stdin_failed("late close failure", now),
            3 => e.on_stderr_line("No conversation found with session ID: old", now),
            4 => {
                now += 5000;
                e.on_tick(now)
            }
            _ => e.on_stderr_line("No message found with message.uuid of: old", now),
        };
        assert!(
            !a.iter().any(|a| matches!(
                a,
                Action::Spawn(_)
                    | Action::Done(_)
                    | Action::RespondJson(_, _)
                    | Action::StartSse
                    | Action::Emit(_)
            )),
            "response/retry after result: {a:?}"
        );
        actions.extend(a);
    }
    actions.extend(terminal(&mut e, now));
    let actual = final_effect(&actions);
    let mixed = t.bare.contains(&true) && t.bare.contains(&false);
    if mixed || t.invalid != 0 {
        assert_eq!(actual, MapEffect::Discard);
        TOOL_COUNTS[if mixed { 2 } else { 3 }].fetch_add(1, Ordering::Relaxed);
    } else {
        let mut history = req(t.stream).messages;
        history.push(clod_core::request::Message {
            role: "assistant".into(),
            content: clod_core::request::Content::Blocks(expected),
        });
        // All generated blocks use exactly canonical client fields; sorted-key JSON
        // serialization of these known shapes is an independent hash reference.
        let hash: [u8; 32] = Sha256::digest(serde_json::to_vec(&history).unwrap()).into();
        let cp = Checkpoint {
            session_id: "fork-two".into(),
            resume_uuid: format!("record-{}", t.bare.len() - 1),
            pending_tool_ids: (0..t.bare.len()).map(|i| format!("call-{i}")).collect(),
            prefix_hash: hash,
        };
        assert_eq!(actual, MapEffect::Commit(cp));
        TOOL_COUNTS[usize::from(t.bare[0])].fetch_add(1, Ordering::Relaxed);
        if t.bare.len() > 1 {
            TOOL_COUNTS[4].fetch_add(1, Ordering::Relaxed);
        }
    }
    assert!(e.on_tick(now + 900000).is_empty());
    assert!(e.on_client_gone(now).is_empty());
}

fn check_retry(stream: bool, kind: u8, before: u64, between: u64) {
    let (mut e, _) = setup(stream, true);
    assert!(e
        .on_tick(before)
        .iter()
        .all(|a| !matches!(a, Action::Done(_))));
    let a = e.on_stderr_line("No conversation found with session ID: source", before);
    assert!(!a.iter().any(|a| matches!(
        a,
        Action::StartSse | Action::RespondJson(_, _) | Action::Emit(_)
    )));
    let now = before + between;
    let a = terminal(&mut e, now);
    if now >= 900000 {
        assert!(!a.iter().any(|a| matches!(a, Action::Spawn(_))));
        assert_eq!(final_effect(&a), MapEffect::Discard);
        return;
    }
    assert_eq!(
        a.iter().filter(|a| matches!(a, Action::Spawn(_))).count(),
        1
    );
    let a = match kind {
        0 => e.on_spawn_failed("retry spawn", now),
        1 => e.on_stderr_line("No message found with message.uuid of: old", now),
        2 => e.on_stdin_failed("retry stdin", now),
        _ => {
            let a = e.on_tick(899999);
            assert!(!a
                .iter()
                .any(|a| matches!(a, Action::CloseStdin | Action::Kill)));
            let a = e.on_tick(900000);
            assert!(a.iter().any(|a| matches!(a, Action::CloseStdin)));
            assert!(a
                .iter()
                .any(|a| matches!(a,Action::WriteStdin(s) if s.contains("interrupt"))));
            a
        }
    };
    assert!(!a.iter().any(|a| matches!(a, Action::Spawn(_))));
    let mut all = a;
    if kind != 0 {
        all.extend(terminal(&mut e, if kind == 3 { 900000 } else { now }));
    }
    assert_eq!(final_effect(&all), MapEffect::Discard);
    assert!(e.on_tick(1800000).is_empty());
}
fn check_timeout(stream: bool, event: bool, result: bool) {
    let (mut e, _) = setup(stream, true);
    if event {
        send(
            &mut e,
            ev(json!({"type":"message_start","message":{"id":"m","content":[]}})),
            10,
        );
    }
    if result {
        e.on_stdout_line(&line(6), 20);
        e.on_stdout_line(&line(8), 20);
    }
    let a = e.on_tick(899999);
    if result {
        assert!(a.iter().any(|a| matches!(a, Action::Kill)));
    } else {
        assert!(!a
            .iter()
            .any(|a| matches!(a, Action::CloseStdin | Action::Kill)));
    }
    let a = e.on_tick(900000);
    if result {
        assert!(a.is_empty());
    } else {
        assert!(a.iter().any(|a| matches!(a, Action::CloseStdin)));
        assert!(a
            .iter()
            .any(|a| matches!(a,Action::WriteStdin(s) if s.contains("interrupt"))));
        assert!(e.on_tick(901999).is_empty());
        assert!(e.on_tick(902000).iter().any(|a| matches!(a, Action::Kill)));
    }
    let effect = final_effect(&terminal(&mut e, 902001));
    assert!(matches!(effect, MapEffect::Commit(_)) == result);
    if !result {
        assert_eq!(effect, MapEffect::Keep);
    }
}
fn check_ping(stream: bool, intervals: usize, active: bool) {
    let (mut e, _) = setup(stream, false);
    send(
        &mut e,
        ev(json!({"type":"message_start","message":{"id":"m","content":[]}})),
        0,
    );
    for i in 1..=intervals {
        let now = i as u64 * 15000;
        assert!(!e
            .on_tick(now - 1)
            .iter()
            .any(|a| matches!(a,Action::Emit(v)if v.event=="ping")));
        let a = e.on_tick(now);
        assert_eq!(
            a.iter()
                .filter(|a| matches!(a,Action::Emit(v)if v.event=="ping"))
                .count(),
            usize::from(stream)
        );
        if active {
            send(
                &mut e,
                ev(
                    json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"x"}}),
                ),
                now,
            );
        }
    }
    let now = intervals as u64 * 15000;
    e.on_stdout_line(&line(6), now);
    e.on_stdout_line(&line(8), now);
    assert!(!e
        .on_tick(now + 15000)
        .iter()
        .any(|a| matches!(a,Action::Emit(v)if v.event=="ping")));
}
proptest! {
    #![proptest_config(ProptestConfig { failure_persistence: Some(Box::new(proptest::test_runner::FileFailurePersistence::Direct("tests/lifecycle_round2.proptest-regressions"))), ..ProptestConfig::default() })]
    #[test]fn tool_result_cleanup(t in tool_histories()){check_tool(&t);}
    #[test]fn retry_failure_deadline(stream in any::<bool>(),kind in 0u8..4,before in prop::sample::select(vec![0,1,300000,899998]),between in prop::sample::select(vec![0,1,2000,5000])){check_retry(stream,kind,before,between);}
    #[test]fn ping_liveness(stream in any::<bool>(),intervals in 1usize..30,active in any::<bool>()){check_ping(stream,intervals,active);}
}
#[test]
fn overall_timeout_boundaries() {
    for s in [false, true] {
        for event in [false, true] {
            for result in [false, true] {
                check_timeout(s, event, result);
            }
        }
    }
}
#[test]
fn tool_coverage() {
    use proptest::strategy::ValueTree;
    let mut runner = proptest::test_runner::TestRunner::deterministic();
    for n in &TOOL_COUNTS {
        n.store(0, Ordering::Relaxed);
    }
    for _ in 0..4000 {
        check_tool(&tool_histories().new_tree(&mut runner).unwrap().current());
    }
    eprintln!("tool lifecycle [prefixed Commit, bare Commit, mixed Discard, invalid Discard, multi-record Commit]: {:?}",TOOL_COUNTS.iter().map(|n|n.load(Ordering::Relaxed)).collect::<Vec<_>>());
}
#[test]
fn no_retry_after_sse_commit() {
    replay(true, true, &[Op::Out(0), Op::Err(true)], true);
}
#[test]
fn no_retry_after_json_commit() {
    // An ordinary stdin error commits JSON before the stale refusal arrives.
    replay(false, true, &[Op::StdinFailed, Op::Err(true)], true);
}
#[test]
fn explicit_round2() {
    for stream in [false, true] {
        for bare in [
            vec![false],
            vec![false, false, false],
            vec![true, true],
            vec![true, false],
        ] {
            check_tool(&ToolHistory {
                stream,
                resume: true,
                bare,
                payload: "explicit".into(),
                invalid: 0,
                max_turns: true,
                after: vec![3, 0, 1, 2, 4, 5],
            });
        }
        for kind in 0..4 {
            check_retry(stream, kind, 300000, 5000);
        }
    }
}
