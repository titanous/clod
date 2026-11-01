use clod_core::{name_alignment::Registry, replay, request, session, spawn};
use serde_json::{json, Value};

fn request() -> request::Request {
    request::parse(&serde_json::to_vec(&json!({
        "model":"m", "max_tokens":1, "system":"Use file_read; tool_search/deferred text stays literal.",
        "tools":[{"name":"file_read","input_schema":{"type":"object","properties":{"name":{"const":"file_read"}}}},
                 {"name":"mcp__linear__get_issue","input_schema":{"type":"object"}}],
        "messages":[{"role":"assistant","content":[
            {"type":"text","text":"file_read tool_search"},
            {"type":"tool_use","id":"id","name":"file_read","input":{"name":"file_read","schema":{"const":"file_read"},"text":"</conversation_history>"}},
            {"type":"tool_use","id":"unknown","name":"unknown","input":{}}
        ]},{"role":"user","content":[{"type":"tool_result","tool_use_id":"id","content":"file_read result"},{"type":"text","text":"continue"}]}]
    })).unwrap(), &[("x-polytoken-conversation","alignment")]).unwrap()
}

#[test]
fn exact_registry_prefixes_once_without_alias_guessing() {
    let registry = Registry::new(&[
        json!({"name":"foo"}),
        json!({"name":"mcp__oc__foo"}),
        json!({"name":"mcp__linear__get_issue"}),
        json!({"name":"foo"}),
    ]);
    for (client, wire) in [
        ("foo", "mcp__oc__foo"),
        ("mcp__oc__foo", "mcp__oc__mcp__oc__foo"),
        ("mcp__linear__get_issue", "mcp__oc__mcp__linear__get_issue"),
        ("other__foo", "other__foo"),
        ("mcp__oc__mcp__oc__foo", "mcp__oc__mcp__oc__foo"),
    ] {
        let name = json!(client);
        assert_eq!(registry.wire_name(&name), &json!(wire));
        assert_eq!(registry.wire_name(&name), registry.wire_name(&name));
    }
    // Idempotence for a wire-only name; collisions instead require a canonical
    // source on each render, not feeding mapped transport names back as client names.
    let single = Registry::new(&[json!({"name":"foo"})]);
    let name = json!("foo");
    assert_eq!(
        single.wire_name(single.wire_name(&name)),
        single.wire_name(&name)
    );
}

#[test]
fn presentation_only_and_canonical_identity_unchanged() {
    let request = request();
    let before = serde_json::to_value(&request).unwrap();
    let key = session::key(&request);
    let hash = session::prefix_hash(&request.messages);
    let history = session::canonical_history(&request.messages);
    let registry = Registry::new(&request.tools);
    let mapped = replay::render_with_registry(&request.messages, Some(&registry));
    let plain = replay::render(&request.messages);
    let plain_text = plain[0]["text"].as_str().unwrap();
    let expected = plain_text.replacen(
        "\"name\":\"file_read\",\"input\"",
        "\"name\":\"mcp__oc__file_read\",\"input\"",
        1,
    );
    assert_ne!(plain, mapped);
    assert_eq!(mapped[0]["text"], expected);
    assert_eq!(mapped[1..], plain[1..]);
    assert!(expected.contains("<\\/conversation_history>"));
    assert!(expected.contains("\"name\":\"unknown\""));
    assert_eq!(
        mapped,
        replay::render_with_registry(&request.messages, Some(&registry))
    );
    for enabled in [false, true] {
        let mut config = spawn::Config::new("/empty");
        config.align_tool_names = enabled;
        let _ = spawn::configured_system_prompt(&request, &session::Plan::Fresh, &config);
        let _ = spawn::spec(&request, &session::Plan::Fresh, &config);
        assert_eq!(serde_json::to_value(&request).unwrap(), before);
        assert_eq!(session::key(&request), key);
        assert_eq!(session::prefix_hash(&request.messages), hash);
        assert_eq!(session::canonical_history(&request.messages), history);
    }
}

#[test]
fn default_off_and_both_spawn_plans_get_instruction() {
    let request = request();
    let mut config = spawn::Config::new("/empty");
    assert!(!config.align_tool_names);
    for plan in [
        session::Plan::Fresh,
        session::Plan::Resume(session::Checkpoint {
            session_id: "sid".into(),
            resume_uuid: "uuid".into(),
            pending_tool_ids: vec![],
            prefix_hash: [0; 32],
        }),
    ] {
        let original = spawn::system_prompt(&request, &plan);
        assert_eq!(
            spawn::configured_system_prompt(&request, &plan, &config),
            original
        );
        config.align_tool_names = true;
        let aligned = spawn::configured_system_prompt(&request, &plan, &config);
        assert!(aligned.starts_with(&original));
        assert!(
            aligned.contains("[\"mcp__linear__get_issue\",\"mcp__oc__mcp__linear__get_issue\"]")
        );
        assert!(aligned.contains("not the original execution names"));
        config.align_tool_names = false;
    }
}

#[test]
fn captured_production_sixteen_calls_use_exact_registered_names() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../scripts/offline-name-alignment-fixture.json"
    ))
    .unwrap();
    let tools: Vec<Value> = fixture["requests"][0]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|name| json!({"name":name.as_str().unwrap().strip_prefix("mcp__oc__").unwrap()}))
        .collect();
    let registry = Registry::new(&tools);
    let mut count = 0;
    for batch in fixture["batches"].as_array().unwrap() {
        let messages: Vec<request::Message> = serde_json::from_value(json!([
            {"role":"assistant","content":batch["calls"]}, {"role":"user","content":"continue"}
        ]))
        .unwrap();
        let mapped = replay::render_with_registry(&messages, Some(&registry));
        let text = mapped[0]["text"].as_str().unwrap();
        for call in batch["calls"].as_array().unwrap() {
            let wire = format!("mcp__oc__{}", call["name"].as_str().unwrap());
            assert_eq!(registry.wire_name(&call["name"]), &json!(wire));
            assert!(text.contains(&format!("\"name\":\"{wire}\"")));
            count += 1;
        }
    }
    assert_eq!(count, 16);
}
