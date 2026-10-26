use clod_core::{request, session};
use serde_json::json;

#[test]
fn tool_schema_annotation_named_properties_remain_significant() {
    let body = json!({"model":"m","max_tokens":1,"messages":[],"tools":[{"name":"read","input_schema":{"type":"object","properties":{"cache_control":{"type":"string"}}}}]});
    let a = request::parse(
        &serde_json::to_vec(&body).unwrap(),
        &[("x-polytoken-conversation", "c")],
    )
    .unwrap();
    let mut b = a.clone();
    b.tools[0]["input_schema"]["properties"]["cache_control"]["type"] = json!("number");
    assert_ne!(
        session::key(&a),
        session::key(&b),
        "changing a tool parameter's type must invalidate the checkpoint key"
    );
}

#[test]
fn tool_schema_content_defaults_remain_significant() {
    let body = json!({"model":"m","max_tokens":1,"messages":[],"tools":[{"name":"read","input_schema":{"type":"object","default":{"content":[{"type":"thinking","value":1}]}}}]});
    let a = request::parse(
        &serde_json::to_vec(&body).unwrap(),
        &[("x-polytoken-conversation", "c")],
    )
    .unwrap();
    let mut b = a.clone();
    b.tools[0]["input_schema"]["default"]["content"][0]["value"] = json!(2);
    assert_ne!(
        session::key(&a),
        session::key(&b),
        "schema defaults are data, not message blocks"
    );
}
