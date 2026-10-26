//! Anthropic SSE wire encoding and error bodies.
use alloc::{format, string::String};
use serde_json::{json, Value};

#[derive(Clone, Debug, PartialEq)]
pub struct SseEvent {
    pub event: String,
    pub data: Value,
}
impl SseEvent {
    pub fn new(event: impl Into<String>, data: Value) -> Self {
        Self {
            event: event.into(),
            data,
        }
    }
    pub fn encode(&self) -> String {
        encode(&self.event, &self.data)
    }
}
/// Event names are supplied by the core, never by a client.
pub fn encode(event: &str, data: &Value) -> String {
    format!("event: {event}\ndata: {data}\n\n")
}
pub fn error_envelope(error_type: &str, message: &str) -> Value {
    json!({"type":"error","error":{"type":error_type,"message":message}})
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sse_wire_encoding() {
        assert_eq!(
            encode("ping", &json!({"type":"ping"})),
            "event: ping\ndata: {\"type\":\"ping\"}\n\n"
        );
        let event = SseEvent::new("error", error_envelope("api_error", "bad\n\"value\""));
        assert_eq!(event.encode().lines().count(), 3);
        assert!(event.encode().contains("bad\\n\\\"value\\\""));
    }
    #[test]
    fn error_json_shape() {
        assert_eq!(
            error_envelope("invalid_request_error", "bad"),
            json!({"type":"error","error":{"type":"invalid_request_error","message":"bad"}})
        );
    }
}
