//! The captured CLI NDJSON subset. Unrecognized records retain their payload.
use alloc::{
    string::{String, ToString},
    vec::Vec,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ControlRequest {
    pub request_id: String,
    pub request: Value,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Assistant {
    #[serde(default)]
    pub uuid: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    pub message: Value,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ResultRecord {
    pub subtype: String,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub errors: Vec<String>,
    #[serde(default)]
    pub result: Option<String>,
    #[serde(default)]
    pub usage: Value,
}
#[derive(Clone, Debug)]
pub enum Record {
    ControlRequest(ControlRequest),
    StreamEvent {
        event: Value,
        session_id: Option<String>,
    },
    Assistant(Assistant),
    Result(ResultRecord),
    System(Value),
    Unknown(Value),
}
pub fn parse(line: &str) -> core::result::Result<Record, serde_json::Error> {
    let value: Value = serde_json::from_str(line)?;
    Ok(match value["type"].as_str() {
        Some("control_request") => Record::ControlRequest(serde_json::from_value(value)?),
        Some("assistant") => Record::Assistant(serde_json::from_value(value)?),
        Some("result") => Record::Result(serde_json::from_value(value)?),
        Some("stream_event") => Record::StreamEvent {
            event: value["event"].clone(),
            session_id: value["session_id"].as_str().map(String::from),
        },
        Some("system") => Record::System(value),
        _ => Record::Unknown(value),
    })
}
pub fn control(id: &str, request: Value) -> String {
    json!({"type":"control_request","request_id":id,"request":request}).to_string()
}
pub fn response(id: &str, response: Value) -> String {
    json!({"type":"control_response","response":{"subtype":"success","request_id":id,"response":response}}).to_string()
}
/// SDK declaration only; not verified by the live capture.
pub fn interrupt(id: &str) -> String {
    control(id, json!({"subtype":"interrupt"}))
}
pub fn deny(id: &str) -> String {
    response(
        id,
        json!({"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"Forwarded to client. End your turn."}}),
    )
}
