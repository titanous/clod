use clod_core::{request, session, spawn};
use serde_json::json;

fn request() -> request::Request {
    request::parse(
        &serde_json::to_vec(&json!({"model":"m","max_tokens":1,"messages":[],"system":[{"type":"text","text":"system"}],"tools":[{"name":"a","input_schema":{"type":"object"}},{"name":"b","input_schema":{"type":"object"}}]})).unwrap(),
        &[("x-polytoken-conversation", "c")],
    ).unwrap()
}

#[test]
fn tool_level_cache_annotations_do_not_change_key() {
    let original = request();
    for index in 0..2 {
        for annotation in [
            json!({"type":"ephemeral"}),
            json!({"type":"ephemeral","ttl":"5m"}),
            json!({"type":"ephemeral","ttl":"1h"}),
        ] {
            let mut annotated = original.clone();
            annotated.tools[index]["cache_control"] = annotation;
            assert_eq!(session::key(&original), session::key(&annotated));
        }
    }
}

#[test]
fn system_text_and_nested_metadata_have_distinct_roles() {
    let original = request();
    let mut nested = original.clone();
    nested.system = request::Content::Blocks(vec![
        json!({"type":"text","text":"system","extra":{"cache_control":"arbitrary","content":[{"type":"thinking","value":1}]}}),
    ]);
    // Nested fields are not passed to the CLI: only each block's text is used.
    assert_eq!(
        spawn::system_prompt(&original, &session::Plan::Fresh),
        spawn::system_prompt(&nested, &session::Plan::Fresh)
    );
    let mut changed = original.clone();
    changed.system = request::Content::Blocks(vec![json!({"type":"text","text":"changed"})]);
    assert_ne!(session::key(&original), session::key(&changed));
}

#[test]
fn every_accepted_system_block_text_remains_significant() {
    let mut original = request();
    original.system = request::Content::Blocks(vec![json!({"type":"thinking","text":"first"})]);
    let mut changed = original.clone();
    changed.system = request::Content::Blocks(vec![json!({"type":"thinking","text":"second"})]);
    assert_ne!(
        spawn::system_prompt(&original, &session::Plan::Fresh),
        spawn::system_prompt(&changed, &session::Plan::Fresh)
    );
    assert_ne!(session::key(&original), session::key(&changed));
}
