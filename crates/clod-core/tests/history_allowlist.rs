use clod_core::{
    request::{self, Content},
    session::{self, Checkpoint, Plan, SessionMap},
};
use serde_json::{json, Value};

fn request(block: Value) -> request::Request {
    request::parse(&serde_json::to_vec(&json!({"model":"m","max_tokens":8,"messages":[{"role":"assistant","content":[block]},{"role":"user","content":"next"}]})).unwrap(), &[("x-polytoken-conversation","c")]).unwrap()
}
fn map(r: &request::Request) -> SessionMap {
    let mut map = SessionMap::default();
    map.insert(
        session::key(r).unwrap(),
        Checkpoint {
            session_id: "s".into(),
            resume_uuid: "u".into(),
            pending_tool_ids: vec![],
            prefix_hash: session::prefix_hash(&r.messages[..1]),
        },
    );
    map
}
#[test]
fn extra_known_block_fields_do_not_prevent_resume() {
    for block in [
        json!({"type":"text","text":"hello"}),
        json!({"type":"tool_use","id":"t","name":"read","input":{"a":1}}),
        json!({"type":"tool_result","tool_use_id":"t","content":"ok"}),
    ] {
        let mut r = request(block);
        let map = map(&r);
        let Content::Blocks(blocks) = &mut r.messages[0].content else {
            panic!()
        };
        blocks[0]["future_provider_field"] = json!({"arbitrary":true});
        assert!(matches!(session::plan(&r, &map), Plan::Resume(_)));
    }
}
#[test]
fn significant_fields_still_prevent_resume() {
    let original = json!({"type":"tool_use","id":"t","name":"read","input":{"a":1,"cache_control":"significant","content":[{"type":"thinking","value":1}]}});
    for (field, value) in [
        ("id", json!("other")),
        ("name", json!("write")),
        ("input", json!({"a":2})),
    ] {
        let mut r = request(original.clone());
        let map = map(&r);
        let Content::Blocks(blocks) = &mut r.messages[0].content else {
            panic!()
        };
        blocks[0][field] = value;
        assert_eq!(session::plan(&r, &map), Plan::Fresh);
    }
    let a = request(original.clone());
    let mut nested = a.clone();
    let Content::Blocks(blocks) = &mut nested.messages[0].content else {
        panic!()
    };
    blocks[0]["input"]["cache_control"] = json!("changed");
    assert_ne!(
        session::prefix_hash(&a.messages),
        session::prefix_hash(&nested.messages)
    );
    let mut nested = a.clone();
    let Content::Blocks(blocks) = &mut nested.messages[0].content else {
        panic!()
    };
    blocks[0]["input"]["content"][0]["value"] = json!(2);
    assert_ne!(
        session::prefix_hash(&a.messages),
        session::prefix_hash(&nested.messages)
    );
    let mut r = request(json!({"type":"text","text":"hello"}));
    let map = map(&r);
    let Content::Blocks(blocks) = &mut r.messages[0].content else {
        panic!()
    };
    blocks[0]["text"] = json!("changed");
    assert_eq!(session::plan(&r, &map), Plan::Fresh);
}
#[test]
fn defaults_unknown_blocks_and_keys_are_conservative() {
    let r = request(json!({"type":"tool_result","tool_use_id":"t","content":"ok"}));
    let mut explicit = r.clone();
    let Content::Blocks(blocks) = &mut explicit.messages[0].content else {
        panic!()
    };
    blocks[0]["is_error"] = json!(false);
    assert_eq!(
        session::prefix_hash(&r.messages),
        session::prefix_hash(&explicit.messages)
    );
    let mut r = request(json!({"type":"future","extra":1}));
    let map = map(&r);
    let Content::Blocks(blocks) = &mut r.messages[0].content else {
        panic!()
    };
    blocks[0]["extra"] = json!(2);
    assert_eq!(session::plan(&r, &map), Plan::Fresh);
    let mut a = request(json!({"type":"text","text":"x"}));
    a.tools = vec![json!({"name":"read","input_schema":{"type":"object","caller":1}})];
    let mut b = a.clone();
    b.tools[0]["input_schema"]["caller"] = json!(2);
    assert_ne!(session::key(&a), session::key(&b));
}
