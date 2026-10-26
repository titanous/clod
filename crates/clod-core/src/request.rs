//! The supported Messages request fields, without an HTTP dependency.
use alloc::{
    string::{String, ToString},
    vec::Vec,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const TOOL_PREFIX: &str = "mcp__oc__";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Content {
    Text(String),
    Blocks(Vec<Value>),
}
impl Default for Content {
    fn default() -> Self {
        Self::Blocks(Vec::new())
    }
}
impl Content {
    pub fn blocks(&self) -> &[Value] {
        match self {
            Self::Text(_) => &[],
            Self::Blocks(blocks) => blocks,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: Content,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub model: String,
    #[serde(default)]
    pub system: Content,
    pub messages: Vec<Message>,
    #[serde(default)]
    pub tools: Vec<Value>,
    pub max_tokens: u64,
    #[serde(default)]
    pub thinking: Option<Value>,
    #[serde(default)]
    pub output_config: Option<Value>,
    #[serde(default)]
    pub stream: bool,
    #[serde(skip)]
    pub conversation_id: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid Messages request: {0}")]
    Json(#[from] serde_json::Error),
    #[error("tool definition must have a string name")]
    MissingToolName,
    #[error("tool '{0}' exceeds 64 characters with the mcp__oc__ prefix")]
    ToolNameTooLong(String),
    #[error("tool '{0}' contains commas or whitespace")]
    InvalidToolName(String),
}

pub fn identity(headers: &[(&str, &str)]) -> Option<String> {
    ["x-polytoken-conversation", "x-polytoken-session"]
        .iter()
        .find_map(|wanted| {
            headers
                .iter()
                .filter(|(name, _)| name.eq_ignore_ascii_case(wanted))
                .map(|(_, value)| value.trim())
                .find(|value| !value.is_empty())
                .map(ToString::to_string)
        })
}

pub fn parse(body: &[u8], headers: &[(&str, &str)]) -> Result<Request, Error> {
    let mut request: Request = serde_json::from_slice(body)?;
    request.conversation_id = identity(headers);
    for tool in &request.tools {
        let name = tool
            .get("name")
            .and_then(Value::as_str)
            .ok_or(Error::MissingToolName)?;
        if name.chars().any(|c| c == ',' || c.is_whitespace()) {
            return Err(Error::InvalidToolName(name.to_string()));
        }
        if TOOL_PREFIX.len() + name.chars().count() > 64 {
            return Err(Error::ToolNameTooLong(name.to_string()));
        }
    }
    Ok(request)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn request_subset_and_identity() {
        let mut value = json!({"model":"m","messages":[],"max_tokens":12,"system":"s","ignored":1});
        let request = parse(
            &serde_json::to_vec(&value).unwrap(),
            &[
                ("X-Polytoken-Session", " old "),
                ("x-polytoken-conversation", " new "),
            ],
        )
        .unwrap();
        assert_eq!(request.conversation_id.as_deref(), Some("new"));
        assert!(!request.stream);
        value["system"] = json!([{"type":"text","text":"s"}]);
        assert!(parse(&serde_json::to_vec(&value).unwrap(), &[]).is_ok());
        assert_eq!(
            identity(&[
                ("X-Polytoken-Conversation", " "),
                ("X-Polytoken-Session", " fallback ")
            ])
            .as_deref(),
            Some("fallback")
        );
        assert_eq!(identity(&[("X-Polytoken-Session", " ")]), None);
    }
    #[test]
    fn tool_name_too_long_no_spawn() {
        for (length, valid) in [(55, true), (56, false)] {
            let name = "x".repeat(length);
            let body = json!({"model":"m","messages":[],"max_tokens":1,"tools":[{"name":name}]});
            let result = parse(&serde_json::to_vec(&body).unwrap(), &[]);
            assert_eq!(result.is_ok(), valid);
            if let Err(error) = result {
                assert!(error.to_string().contains(&name));
            }
        }
    }
}
