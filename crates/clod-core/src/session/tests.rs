use super::*;
use alloc::{string::ToString, vec};
fn request() -> Request {
    crate::request::parse(br#"{"model":"m","system":"s","max_tokens":10,"tools":[{"name":"run","description":"execute","input_schema":{"type":"object"}}],"messages":[{"role":"user","content":"hi"},{"role":"assistant","content":[{"type":"tool_use","id":"a","name":"run","input":{}}]},{"role":"user","content":[{"type":"tool_result","tool_use_id":"a","content":"ok"}]}]}"#, &[("X-Polytoken-Conversation","c")]).unwrap()
}
fn checkpoint(request: &Request) -> Checkpoint {
    Checkpoint {
        session_id: "session".into(),
        resume_uuid: "uuid".into(),
        pending_tool_ids: vec!["a".into()],
        prefix_hash: prefix_hash(&request.messages[..2]),
    }
}
fn populated(request: &Request) -> SessionMap {
    let mut map = SessionMap::default();
    map.insert(key(request).unwrap(), checkpoint(request));
    map
}
fn fresh(change: impl FnOnce(&mut Request)) {
    let mut request = request();
    let map = populated(&request);
    change(&mut request);
    assert_eq!(plan(&request, &map), Plan::Fresh);
}
#[test]
fn context_changes_obey_key_mode() {
    let original = request();
    let mut changed = original.clone();
    for change_tools in [false, true] {
        if change_tools {
            changed.tools.clear();
        } else {
            changed.system = Content::Text("new instructions".into());
        }
        assert_ne!(key(&original), key(&changed));
        assert_ne!(
            key_with_mode(&original, KeyMode::Strict),
            key_with_mode(&changed, KeyMode::Strict)
        );
    }
    changed.model = "other".into();
    assert_ne!(
        key_with_mode(&original, KeyMode::Strict),
        key_with_mode(&changed, KeyMode::Strict)
    );
}
#[test]
fn plan_resume_tool_results() {
    let mut request = request();
    let map = populated(&request);
    if let Content::Blocks(blocks) = &mut request.messages[2].content {
        blocks.push(json!({"type":"text","text":"continue"}));
    }
    assert!(matches!(plan(&request, &map), Plan::Resume(_)));
}
#[test]
fn plan_resume_text_after_text() {
    let mut request = request();
    request.messages[1].content = Content::Text("hello".into());
    request.messages[2].content = Content::Text("more".into());
    let mut cp = checkpoint(&request);
    cp.pending_tool_ids.clear();
    let mut map = SessionMap::default();
    map.insert(key(&request).unwrap(), cp);
    assert!(matches!(plan(&request, &map), Plan::Resume(_)));
    request.messages[2].content =
        Content::Blocks(vec![json!({"type":"tool_result","tool_use_id":"a"})]);
    assert_eq!(plan(&request, &map), Plan::Fresh);
}
#[test]
fn plan_fresh_edited() {
    fresh(|r| r.messages[0].content = Content::Text("edited".into()));
}
#[test]
fn plan_fresh_no_header() {
    fresh(|r| r.conversation_id = None);
}
#[test]
fn plan_fresh_empty_map() {
    assert_eq!(plan(&request(), &SessionMap::default()), Plan::Fresh);
}
#[test]
fn plan_fresh_model_change() {
    fresh(|r| r.model = "other".into());
}
#[test]
fn plan_fresh_system_change() {
    fresh(|r| r.system = Content::Text("other".into()));
}
#[test]
fn plan_fresh_tool_name_change() {
    fresh(|r| r.tools[0]["name"] = json!("other"));
}
#[test]
fn plan_fresh_schema_change() {
    fresh(|r| r.tools[0]["input_schema"] = json!({"type":"string"}));
}
#[test]
fn plan_fresh_description_change() {
    fresh(|r| r.tools[0]["description"] = json!("other"));
}
#[test]
fn plan_fresh_partial_results() {
    fresh(|r| r.messages[2].content = Content::Blocks(vec![]));
}
#[test]
fn plan_fresh_duplicate_result() {
    fresh(|r| {
        if let Content::Blocks(blocks) = &mut r.messages[2].content {
            blocks.push(blocks[0].clone());
        }
    });
}
#[test]
fn plan_fresh_unknown_result() {
    fresh(|r| {
        r.messages[2].content =
            Content::Blocks(vec![json!({"type":"tool_result","tool_use_id":"unknown"})])
    });
}
#[test]
fn plan_fresh_text_with_pending() {
    fresh(|r| r.messages[2].content = Content::Text("hi".into()));
}
#[test]
fn plan_fresh_text_before_results() {
    fresh(|r| {
        if let Content::Blocks(blocks) = &mut r.messages[2].content {
            blocks.insert(0, json!({"type":"text","text":"first"}));
        }
    });
}
#[test]
fn plan_fresh_not_single_user_tail() {
    fresh(|r| r.messages[2].role = "assistant".into());
}
#[test]
fn no_checkpoint_on_missing_uuid() {
    let request = request();
    let mut map = SessionMap::default();
    let mut cp = checkpoint(&request);
    cp.resume_uuid.clear();
    map.insert(key(&request).unwrap(), cp);
    assert_eq!(plan(&request, &map), Plan::Fresh);
}
#[test]
fn title_request_separate_key() {
    fresh(|r| {
        r.system = Content::Text("Generate a title".into());
        r.tools.clear();
    });
}
#[test]
fn key_component_boundaries() {
    let mut a = request();
    let mut b = a.clone();
    a.conversation_id = Some("ab".into());
    a.model = "c".into();
    b.conversation_id = Some("a".into());
    b.model = "bc".into();
    assert_ne!(key(&a), key(&b));
    b = a.clone();
    b.tools.push(json!({"name":"second","input_schema":{}}));
    a.tools = b.tools.iter().rev().cloned().collect();
    assert_eq!(key(&a), key(&b));
}
#[test]
fn canonical_hash_ignores_thinking_and_cache_control() {
    let a = json!([{"role":"assistant","content":[{"type":"text","text":"ok","cache_control":{"type":"ephemeral"}},{"type":"thinking","thinking":"secret"},{"type":"redacted_thinking","data":"x"}]}]);
    let b = json!([{"content":[{"text":"ok","type":"text"}],"role":"assistant"}]);
    assert_eq!(hash(&a), hash(&b));
    assert_ne!(
        hash(&json!({"input":{"type":"thinking","value":1}})),
        hash(&json!({"input":{"type":"thinking","value":2}}))
    );
    assert_eq!(canonical_json(&json!({"z":1,"a":2})), br#"{"a":2,"z":1}"#);
}
#[test]
fn map_effects_and_lru() {
    let request = request();
    let cp = checkpoint(&request);
    let mut map = SessionMap::new(2);
    for k in [1, 2] {
        apply(
            &mut map,
            Final {
                key: Some([k; 32]),
                map_effect: MapEffect::Commit(cp.clone()),
            },
        );
    }
    map.touch(&[1; 32]);
    map.insert([3; 32], cp.clone());
    assert!(map.get(&[2; 32]).is_none());
    assert!(map.get(&[1; 32]).is_some());
    apply(
        &mut map,
        Final {
            key: Some([1; 32]),
            map_effect: MapEffect::Keep,
        },
    );
    assert_eq!(map.len(), 2);
    apply(
        &mut map,
        Final {
            key: Some([1; 32]),
            map_effect: MapEffect::Discard,
        },
    );
    assert_eq!(map.len(), 1);
    apply(
        &mut map,
        Final {
            key: None,
            map_effect: MapEffect::Commit(cp.clone()),
        },
    );
    assert_eq!(map.len(), 1);
    let mut zero = SessionMap::new(0);
    zero.insert([0; 32], cp);
    assert!(zero.is_empty());
}
#[test]
fn different_keys_never_overwrite() {
    let mut request = request();
    let mut map = populated(&request);
    let first = key(&request).unwrap();
    request.conversation_id = Some("second".to_string());
    map.insert(key(&request).unwrap(), checkpoint(&request));
    assert_eq!(map.len(), 2);
    assert!(map.get(&first).is_some());
}
#[test]
fn plan_parallel_results_exact_set() {
    let mut request = request();
    let mut cp = checkpoint(&request);
    cp.pending_tool_ids = vec!["a".into(), "b".into(), "c".into()];
    let mut map = SessionMap::default();
    map.insert(key(&request).unwrap(), cp);
    request.messages[2].content = Content::Blocks(vec![
        json!({"type":"tool_result","tool_use_id":"c","content":"three"}),
        json!({"type":"tool_result","tool_use_id":"a","content":"one"}),
        json!({"type":"tool_result","tool_use_id":"b","content":"two"}),
        json!({"type":"text","text":"continue"}),
    ]);
    assert!(matches!(plan(&request, &map), Plan::Resume(_)));
    if let Content::Blocks(blocks) = &mut request.messages[2].content {
        blocks.remove(1);
    }
    assert_eq!(plan(&request, &map), Plan::Fresh);
}
#[test]
fn planning_is_read_only() {
    let request = request();
    let mut map = SessionMap::new(2);
    let first = key(&request).unwrap();
    map.insert(first, checkpoint(&request));
    map.insert([2; 32], checkpoint(&request));
    assert!(matches!(plan(&request, &map), Plan::Resume(_)));
    map.insert([3; 32], checkpoint(&request));
    assert!(map.get(&first).is_none());
}
#[test]
fn polytoken_followup_hash_matches() {
    let rows: Vec<Value> = include_str!("../../tests/fixtures/polytoken/daemon.ndjson")
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .filter(|row: &Value| row["label"] == "main" && row["path"] == "/v1/messages")
        .collect();
    assert_eq!(rows.len(), 3);
    let requests: Vec<Request> = rows
        .iter()
        .map(|row| {
            let body = serde_json::to_vec(&row["body"]).unwrap();
            crate::request::parse(
                &body,
                &[(
                    "x-polytoken-conversation",
                    row["headers"]["x-polytoken-conversation"].as_str().unwrap(),
                )],
            )
            .unwrap()
        })
        .collect();
    let responses = [
        vec![
            json!({"type":"thinking","thinking":"I will read the harmless fixture.","signature":"harness-signature-v1"}),
            json!({"type":"text","text":"Reading the fixture."}),
            json!({"type":"tool_use","id":"read_1","name":"file_read","input":{"path":"hello.txt"}}),
        ],
        vec![json!({"type":"text","text":"The fixture says hello from the harness."})],
    ];
    for (index, pair) in requests.windows(2).enumerate() {
        assert_eq!(key(&pair[0]), key(&pair[1]), "context key pair {index}");
        // exchange.rs commits request history plus the returned assistant blocks.
        let mut history = pair[0].messages.clone();
        history.push(crate::request::Message {
            role: "assistant".into(),
            content: Content::Blocks(responses[index].clone()),
        });
        let prior = &pair[1].messages[..pair[1].messages.len() - 1];
        assert_eq!(
            prefix_hash(prior),
            prefix_hash(&history),
            "echo differs for pair {index}: echoed={prior:?}, reconstructed={history:?}"
        );
    }
}
