//! Offline constructive turns based on c-parallel and thinking fixtures.
//! Expected content is generated before driving Exchange, not reconstructed by it.
use clod_core::{
    exchange::{Action, Exchange},
    request::{self, Request},
    session::{self, MapEffect, Plan, SessionMap},
    spawn::Config,
    sse,
};
use proptest::prelude::*;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

fn request(stream: bool, messages: Value) -> Request {
    request::parse(&serde_json::to_vec(&json!({"model":"haiku","max_tokens":1000,"stream":stream,
        "messages":messages,"tools":[{"name":"lookup","description":"Lookup","input_schema":{"type":"object"}}]})).unwrap(),
        &[("x-polytoken-conversation", "property")]).unwrap()
}
fn initial() -> Value {
    json!([{"role":"user","content":"hello"}])
}
fn event(v: Value) -> Value {
    json!({"type":"stream_event","session_id":"fork","event":v})
}
fn hook(id: &str) -> Value {
    json!({"type":"control_request","request_id":id,"request":{"subtype":"hook_callback","callback_id":"hook_0","input":{"hook_event_name":"PreToolUse"}}})
}

#[derive(Clone, Debug)]
struct Turn {
    // 0 text, 1 thinking, 2 prefixed lookup, 3 bare lookup.
    kinds: Vec<u8>,
    payload: String,
    cuts: Vec<usize>,
    // 0 valid; 1 mismatched ID; 2 missing final UUID; 3 trailing text;
    // 4 truncated JSON; 5 duplicate message_start.
    invalid: u8,
}
fn turns() -> impl Strategy<Value = Turn> {
    (
        prop::collection::vec(0u8..4, 1..7),
        ".{0,24}",
        prop::collection::vec(0usize..80, 0..12),
        0u8..6,
    )
        .prop_map(|(mut kinds, payload, cuts, invalid)| {
            // End tool turns at a tool block, matching actual checkpoint records.
            if kinds.iter().any(|k| *k >= 2) && *kinds.last().unwrap() < 2 {
                kinds.push(2);
            }
            Turn {
                kinds,
                payload,
                cuts,
                invalid,
            }
        })
}
impl Turn {
    fn build(&self) -> (Vec<Value>, Vec<Value>) {
        let mut lines = vec![hook("before")];
        for (i, method) in [
            "initialize",
            "notifications/initialized",
            "tools/list",
            "tools/call",
        ]
        .iter()
        .enumerate()
        {
            let mut m = json!({"jsonrpc":"2.0","method":method});
            if i != 1 {
                m["id"] = json!(i);
            }
            lines.push(json!({"type":"control_request","request_id":format!("mcp-{i}"),"request":{"subtype":"mcp_message","server_name":"oc","message":m}}));
        }
        lines.push(event(json!({"type":"message_start","message":{"id":"msg","type":"message","role":"assistant","model":"haiku","content":[]}})));
        lines.push(hook("during"));
        let mut expected = vec![];
        for (i, kind) in self.kinds.iter().enumerate() {
            let id = format!("tool-{i}");
            let text = format!("é🦀\n\r\\\"{}", self.payload);
            let input = json!({"key":text});
            let (start, final_block, deltas) = match kind {
                0 => (
                    json!({"type":"text","text":""}),
                    json!({"type":"text","text":text}),
                    vec![json!({"type":"text_delta","text":text})],
                ),
                1 => (
                    json!({"type":"thinking","thinking":""}),
                    json!({"type":"thinking","thinking":text,"signature":"signed"}),
                    vec![
                        json!({"type":"thinking_delta","thinking":text}),
                        json!({"type":"signature_delta","signature":"signed"}),
                    ],
                ),
                _ => {
                    let name = if *kind == 2 {
                        "mcp__oc__lookup"
                    } else {
                        "lookup"
                    };
                    let raw = input.to_string();
                    // JSON string fragments must be valid UTF-8; arbitrary byte cuts
                    // inside a codepoint cannot be represented by stream-json.
                    let mut cuts = vec![0, raw.len()];
                    cuts.extend(
                        self.cuts
                            .iter()
                            .map(|c| c % (raw.len() + 1))
                            .filter(|c| raw.is_char_boundary(*c)),
                    );
                    cuts.sort_unstable();
                    let mut fragments = vec![String::new()];
                    fragments.extend(cuts.windows(2).map(|w| raw[w[0]..w[1]].to_owned()));
                    if self.invalid == 4 {
                        fragments
                            .iter_mut()
                            .rev()
                            .find(|s| !s.is_empty())
                            .unwrap()
                            .pop();
                    }
                    (
                        json!({"type":"tool_use","id":id,"name":name,"input":{}}),
                        json!({"type":"tool_use","id":id,"name":"lookup","input":input}),
                        fragments
                            .into_iter()
                            .map(|s| json!({"type":"input_json_delta","partial_json":s}))
                            .collect(),
                    )
                }
            };
            lines.push(event(
                json!({"type":"content_block_start","index":i,"content_block":start}),
            ));
            for delta in deltas {
                lines.push(event(
                    json!({"type":"content_block_delta","index":i,"delta":delta}),
                ));
            }
            lines.push(event(json!({"type":"content_block_stop","index":i})));
            let mut recorded = final_block.clone();
            if *kind == 2 {
                recorded["name"] = json!("mcp__oc__lookup");
            }
            if self.invalid == 1 && *kind >= 2 {
                recorded["id"] = json!(format!("wrong-{i}"));
            }
            let uuid = if self.invalid == 2 && i == self.kinds.len() - 1 {
                Value::Null
            } else {
                json!(format!("uuid-{i}"))
            };
            lines.push(json!({"type":"assistant","uuid":uuid,"session_id":"fork","message":{"content":[recorded]}}));
            expected.push(final_block);
        }
        if self.invalid == 3 && self.kinds.iter().any(|k| *k >= 2) {
            lines.push(json!({"type":"assistant","uuid":"trailing","session_id":"fork","message":{"content":[{"type":"text","text":"later"}]}}));
        }
        if self.invalid == 5 {
            lines.push(event(
                json!({"type":"message_start","message":{"id":"second","content":[]}}),
            ));
        }
        lines.push(event(json!({"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":1}})));
        lines.push(event(json!({"type":"message_stop"})));
        lines.push(hook("after"));
        let subtype = if self.invalid != 5
            && self.kinds.iter().any(|k| *k >= 2)
            && self.cuts.len().is_multiple_of(2)
        {
            "error_max_turns"
        } else {
            "success"
        };
        lines.push(json!({"type":"result","subtype":subtype,"session_id":"fork","usage":{}}));
        (lines, expected)
    }
}
fn run(lines: &[Value], stream: bool) -> Vec<(usize, Action)> {
    let (mut e, _) = Exchange::new(
        request(stream, initial()),
        &SessionMap::default(),
        Config::new("/empty"),
        0,
    );
    let mut actions = vec![];
    for (i, line) in lines.iter().enumerate() {
        actions.extend(
            e.on_stdout_line(&line.to_string(), i as u64)
                .into_iter()
                .map(|a| (i, a)),
        );
    }
    for a in e
        .on_stdout_eof(100)
        .into_iter()
        .chain(e.on_stderr_eof(100))
        .chain(e.on_exit(0, 100))
    {
        actions.push((lines.len(), a));
    }
    actions
}
// Minimal independent Anthropic-SSE accumulator, keyed by the wire index.
fn accumulate(actions: &[(usize, Action)]) -> Value {
    let mut blocks = BTreeMap::new();
    let mut fragments: BTreeMap<usize, String> = BTreeMap::new();
    let mut stop = Value::Null;
    for (_, action) in actions {
        let Action::Emit(e) = action else { continue };
        let v = &e.data;
        let i = v["index"].as_u64().unwrap_or(0) as usize;
        match e.event.as_str() {
            "content_block_start" => {
                blocks.insert(i, v["content_block"].clone());
            }
            "content_block_delta" => {
                let d = &v["delta"];
                if d["type"] == "input_json_delta" {
                    fragments
                        .entry(i)
                        .or_default()
                        .push_str(d["partial_json"].as_str().unwrap());
                } else {
                    let field = match d["type"].as_str().unwrap() {
                        "text_delta" => "text",
                        "thinking_delta" => "thinking",
                        "signature_delta" => "signature",
                        _ => continue,
                    };
                    let b = blocks.get_mut(&i).unwrap();
                    b[field] = json!(format!(
                        "{}{}",
                        b[field].as_str().unwrap_or(""),
                        d[field].as_str().unwrap()
                    ));
                }
            }
            "message_delta" => stop = v["delta"]["stop_reason"].clone(),
            _ => (),
        }
    }
    for (i, raw) in fragments {
        if !raw.is_empty() {
            if let Ok(input) = serde_json::from_str::<Value>(&raw) {
                blocks.get_mut(&i).unwrap()["input"] = input;
            }
        }
    }
    json!({"content":blocks.into_values().collect::<Vec<_>>(),"stop_reason":stop})
}
fn check(t: &Turn) {
    let (lines, expected) = t.build();
    let streamed = run(&lines, true);
    let buffered = run(&lines, false);
    let message = buffered
        .iter()
        .find_map(|(_, a)| {
            if let Action::RespondJson(200, m) = a {
                Some(m)
            } else {
                None
            }
        })
        .unwrap();
    assert_eq!(
        accumulate(&streamed),
        json!({"content":message["content"],"stop_reason":message["stop_reason"]})
    );
    assert!(!buffered
        .iter()
        .any(|(_, a)| matches!(a, Action::Emit(_) | Action::StartSse)));
    let tools = t.kinds.iter().any(|k| *k >= 2);
    assert_eq!(
        message["stop_reason"],
        if tools { "tool_use" } else { "end_turn" }
    );
    if t.invalid != 4 {
        assert_eq!(message["content"], json!(expected));
    }
    let result = lines.len() - 1;
    for name in ["message_delta", "message_stop"] {
        let terminals: Vec<_> = streamed
            .iter()
            .filter(|(_, a)| matches!(a,Action::Emit(e) if e.event==name))
            .collect();
        assert_eq!(terminals.len(), 1);
        assert_eq!(terminals[0].0, result);
    }
    // Everything except tool-name translation and terminal withholding is exact.
    let expected_events: Vec<_> = lines
        .iter()
        .filter(|l| l["type"] == "stream_event")
        .filter_map(|l| {
            let mut v = l["event"].clone();
            if matches!(v["type"].as_str(), Some("message_delta" | "message_stop")) {
                return None;
            }
            if v["type"] == "message_start" && v["message"]["id"] == "second" {
                return None;
            }
            if v["content_block"]["name"] == "mcp__oc__lookup" {
                v["content_block"]["name"] = json!("lookup");
            }
            Some(v)
        })
        .collect();
    let actual: Vec<_> = streamed
        .iter()
        .filter_map(|(_, a)| {
            if let Action::Emit(e) = a {
                if !matches!(e.event.as_str(), "message_delta" | "message_stop") {
                    Some(e.data.clone())
                } else {
                    None
                }
            } else {
                None
            }
        })
        .collect();
    assert_eq!(actual, expected_events);
    for actions in [&streamed, &buffered] {
        for id in [
            "before", "during", "after", "mcp-0", "mcp-1", "mcp-2", "mcp-3",
        ] {
            let replies: Vec<_> = actions
                .iter()
                .filter_map(|(i, a)| {
                    if let Action::WriteStdin(s) = a {
                        let v: Value = serde_json::from_str(s).unwrap();
                        if v["response"]["request_id"] == id {
                            Some((*i, v))
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                })
                .collect();
            assert_eq!(replies.len(), 1, "reply {id}");
            if id == "during" {
                assert!(lines[replies[0].0]["event"]["type"] == "message_delta" || t.invalid == 5);
            }
            if ["before", "after"].contains(&id) {
                assert_eq!(lines[replies[0].0]["request_id"], id);
            }
            if !id.starts_with("mcp") {
                assert_eq!(
                    replies[0].1["response"]["response"]["hookSpecificOutput"]
                        ["permissionDecision"],
                    "deny"
                );
            } else {
                let m = &replies[0].1["response"]["response"]["mcp_response"];
                assert_eq!(m["jsonrpc"], "2.0");
                match id {
                    "mcp-0" => assert_eq!(m["result"]["serverInfo"]["name"], "oc"),
                    "mcp-1" => assert_eq!(m["result"], json!({})),
                    "mcp-2" => assert_eq!(m["result"]["tools"][0]["name"], "lookup"),
                    _ => assert_eq!(m["result"]["isError"], true),
                }
            }
        }
        let effect = actions
            .iter()
            .find_map(|(_, a)| {
                if let Action::Done(d) = a {
                    Some(&d.map_effect)
                } else {
                    None
                }
            })
            .unwrap();
        let mixed = t.kinds.contains(&2) && t.kinds.contains(&3);
        let invalid = t.invalid == 2 || t.invalid == 5 || (tools && matches!(t.invalid, 1 | 3 | 4));
        if mixed || invalid {
            assert!(!matches!(effect, MapEffect::Commit(_)));
            continue;
        }
        let MapEffect::Commit(cp) = effect else {
            panic!("expected checkpoint: {effect:?}")
        };
        assert_eq!(cp.session_id, "fork");
        assert_eq!(cp.resume_uuid, format!("uuid-{}", t.kinds.len() - 1));
        let pending: Vec<_> = t
            .kinds
            .iter()
            .enumerate()
            .filter(|(_, k)| **k >= 2)
            .map(|(i, _)| format!("tool-{i}"))
            .collect();
        assert_eq!(cp.pending_tool_ids, pending);
        // Reference history hashing for these known shapes: thinking omitted,
        // text and tool fields already client-normalized, sorted JSON keys.
        let history = json!([{"role":"user","content":"hello"},{"role":"assistant","content":expected.iter().filter(|b|b["type"]!="thinking").cloned().collect::<Vec<_>>()}]);
        let normalized = request(false, history).messages;
        let hash: [u8; 32] = Sha256::digest(serde_json::to_vec(&normalized).unwrap()).into();
        assert_eq!(cp.prefix_hash, hash);
        let mut map = SessionMap::default();
        map.insert(
            session::key(&request(false, initial())).unwrap(),
            cp.clone(),
        );
        let tail = if tools {
            json!(cp
                .pending_tool_ids
                .iter()
                .map(|id| json!({"type":"tool_result","tool_use_id":id,"content":"OK"}))
                .collect::<Vec<_>>())
        } else {
            json!("next")
        };
        let mut follow = json!([{"role":"user","content":"hello"},{"role":"assistant","content":expected},{"role":"user","content":tail}]);
        assert!(matches!(
            session::plan(&request(false, follow.clone()), &map),
            Plan::Resume(_)
        ));
        if tools {
            for variant in 0..3 {
                let mut bad = follow.clone();
                let results = bad[2]["content"].as_array_mut().unwrap();
                match variant {
                    0 => {
                        results[0]["tool_use_id"] = json!("unknown");
                    }
                    1 => {
                        results.push(results[0].clone());
                    }
                    _ => {
                        results.pop();
                    }
                }
                assert!(matches!(
                    session::plan(&request(false, bad), &map),
                    Plan::Fresh
                ));
            }
        }
        follow[0]["content"] = json!("edited");
        assert!(matches!(
            session::plan(&request(false, follow), &map),
            Plan::Fresh
        ));
    }
}
proptest! {
    #[test]
    fn generated_turn(t in turns()) { check(&t); }
    #[test]
    fn sse_json_framing(s in any::<String>(), n in any::<i64>(), values in prop::collection::vec(any::<String>(),0..8)) {
        let value=json!({"s":s,"n":n,"values":values});
        let encoded=sse::encode("content_block_delta",&value);
        let frame=encoded.strip_suffix("\n\n").unwrap();
        let (event,data)=frame.split_once('\n').unwrap();
        prop_assert_eq!(event,"event: content_block_delta");
        prop_assert_eq!(serde_json::from_str::<Value>(data.strip_prefix("data: ").unwrap()).unwrap(),value);
    }
    #[test]
    fn arbitrary_lines_do_not_panic(lines in prop::collection::vec(any::<String>(),0..25)) {
        let (mut e,_)=Exchange::new(request(true,initial()),&SessionMap::default(),Config::new("/empty"),0);
        for line in lines { e.on_stdout_line(&line,1); }
    }
}
#[test]
fn coverage_census() {
    use proptest::{
        strategy::ValueTree,
        test_runner::{Config as TestConfig, RngAlgorithm, TestRng, TestRunner},
    };
    let mut runner = TestRunner::new_with_rng(
        TestConfig::default(),
        TestRng::from_seed(RngAlgorithm::ChaCha, &[7; 32]),
    );
    let mut counts = [0usize; 10];
    for _ in 0..2000 {
        let t = turns().new_tree(&mut runner).unwrap().current();
        counts[t.invalid as usize] += 1;
        counts[6] += usize::from(t.kinds.iter().filter(|k| **k >= 2).count() >= 2);
        counts[7] += usize::from(t.kinds.contains(&3) && !t.kinds.contains(&2));
        counts[8] += usize::from(t.kinds.contains(&3) && t.kinds.contains(&2));
        let (lines, _) = t.build();
        counts[9] += usize::from(lines.iter().any(|l| {
            l["event"]["delta"]["partial_json"]
                .as_str()
                .is_some_and(|s| s.contains('é') && !s.contains('🦀'))
        }));
    }
    println!("2000 turns: variants[valid,id,uuid,trailing,truncated,duplicate], parallel-records, bare-only, mixed, separated-unicode = {counts:?}");
    assert!(
        counts.iter().all(|count| *count >= 30),
        "coverage gap: {counts:?}"
    );
}

#[test]
fn structured_malformed_lines_do_not_panic() {
    for v in [
        json!({"type":"content_block_start","index":999,"content_block":{"type":"unknown","data":"é"}}),
        json!({"type":"content_block_delta","index":99,"delta":{"type":"input_json_delta","partial_json":"{"}}),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","name":"undeclared","id":"x","input":{}}}),
        json!({"type":"content_block_delta","index":-1,"delta":null}),
    ] {
        for stream in [false, true] {
            run(
                &[
                    event(v.clone()),
                    json!({"type":"result","subtype":"success"}),
                ],
                stream,
            );
        }
    }
}

#[test]
fn targeted_grammar_cases() {
    for kinds in [
        vec![0],
        vec![1, 2, 2, 2],
        vec![3, 3],
        vec![3, 2],
        vec![0, 1, 2],
    ] {
        for invalid in 0..6 {
            check(&Turn {
                kinds: kinds.clone(),
                payload: "é🦀\n\r\\\"".into(),
                cuts: (0..80).collect(),
                invalid,
            });
        }
    }
}
