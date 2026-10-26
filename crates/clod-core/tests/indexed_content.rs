use clod_core::{
    exchange::{Action, Exchange},
    request,
    session::SessionMap,
    spawn::Config,
};
use serde_json::{json, Value};

#[test]
fn interleaved_content_deltas_update_their_wire_index() {
    let request=request::parse(&serde_json::to_vec(&json!({"model":"haiku","max_tokens":100,"stream":false,"messages":[{"role":"user","content":"hello"}]})).unwrap(),&[]).unwrap();
    let (mut exchange, _) =
        Exchange::new(request, &SessionMap::default(), Config::new("/empty"), 0);
    for event in [
        json!({"type":"message_start","message":{"content":[]}}),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
        json!({"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"first"}}),
        json!({"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"second"}}),
        json!({"type":"content_block_stop","index":1}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"message_delta","delta":{"stop_reason":"end_turn"}}),
        json!({"type":"message_stop"}),
    ] {
        exchange.on_stdout_line(&json!({"type":"stream_event","event":event}).to_string(), 1);
    }
    let actions =
        exchange.on_stdout_line(&json!({"type":"result","subtype":"success"}).to_string(), 2);
    let message: &Value = actions
        .iter()
        .find_map(|a| {
            if let Action::RespondJson(200, v) = a {
                Some(v)
            } else {
                None
            }
        })
        .unwrap();
    assert_eq!(
        message["content"],
        json!([{"type":"text","text":"first"},{"type":"text","text":"second"}])
    );
}
