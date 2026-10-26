//! Canonical history, conservative resume planning, and explicit map effects.
use crate::request::{Content, Message, Request};
use alloc::{
    collections::{BTreeMap, BTreeSet},
    string::String,
    vec::Vec,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub type Hash = [u8; 32];
pub type SessionKey = Hash;

/// Canonicalize JSON recursively. Only content blocks are omitted; tool schemas
/// and inputs may legitimately contain objects with a `type` property.
fn clean(value: &Value, in_blocks: bool) -> Value {
    match value {
        Value::Object(object) => Value::Object(
            object
                .iter()
                .filter(|(key, _)| key.as_str() != "cache_control")
                .map(|(key, value)| (key.clone(), clean(value, key == "content")))
                .collect(),
        ),
        Value::Array(array) => Value::Array(
            array
                .iter()
                .filter(|item| {
                    !in_blocks
                        || !matches!(
                            item.get("type").and_then(Value::as_str),
                            Some("thinking" | "redacted_thinking")
                        )
                })
                .map(|item| clean(item, false))
                .collect(),
        ),
        _ => value.clone(),
    }
}
/// Sorted object keys, compact JSON, no cache annotations or thinking blocks.
pub fn canonical_json(value: &Value) -> Vec<u8> {
    // serde_json's default map representation is a sorted BTreeMap.
    serde_json::to_vec(&clean(value, true)).expect("JSON Value serialization is infallible")
}
fn hash(value: &Value) -> Hash {
    Sha256::digest(canonical_json(value)).into()
}
/// History-only normalization. Polytoken's Anthropic serializer echoes text and
/// L28-58 and L161-207; tool_result fields/default false are at L131-158.
/// Images contain source (L87-98). Do not apply this to system/tools key hashing,
/// or recurse into tool input/schema JSON: those values remain fully significant.
fn history_block(block: &Value) -> Option<Value> {
    let fields: &[&str] = match block.get("type").and_then(Value::as_str) {
        Some("thinking" | "redacted_thinking") => return None,
        Some("text") => &["type", "text"],
        Some("tool_use") => &["type", "id", "name", "input"],
        Some("tool_result") => &["type", "tool_use_id", "content", "is_error"],
        Some("image") => &["type", "source"],
        // Unknown blocks fail toward replay rather than losing significant data.
        _ => return Some(block.clone()),
    };
    let mut result = serde_json::Map::new();
    for field in fields {
        if let Some(value) = block.get(*field) {
            result.insert((*field).into(), value.clone());
        } else if *field == "is_error" {
            result.insert((*field).into(), Value::Bool(false));
        }
    }
    if result.get("type").and_then(Value::as_str) == Some("tool_result") {
        if let Some(Value::Array(content)) = result.get_mut("content") {
            *content = content.iter().filter_map(history_block).collect();
        }
    }
    Some(Value::Object(result))
}
pub fn canonical_history(messages: &[Message]) -> Vec<u8> {
    let mut messages = messages.to_vec();
    for message in &mut messages {
        if let Content::Blocks(blocks) = &mut message.content {
            *blocks = blocks.iter().filter_map(history_block).collect();
        }
    }
    serde_json::to_vec(&messages).expect("message serialization is infallible")
}
pub fn prefix_hash(messages: &[Message]) -> Hash {
    Sha256::digest(canonical_history(messages)).into()
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum KeyMode {
    #[default]
    Strict,
}
pub fn key(request: &Request) -> Option<SessionKey> {
    key_with_mode(request, KeyMode::Strict)
}
pub fn key_with_mode(request: &Request, _mode: KeyMode) -> Option<SessionKey> {
    let id = request.conversation_id.as_ref()?;
    let mut tools = request.tools.clone();
    tools.sort_by(|a, b| {
        a.get("name")
            .and_then(Value::as_str)
            .cmp(&b.get("name").and_then(Value::as_str))
    });
    Some(hash(&json!([id, request.model, request.system, tools])))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Checkpoint {
    pub session_id: String,
    pub resume_uuid: String,
    pub pending_tool_ids: Vec<String>,
    pub prefix_hash: Hash,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Plan {
    Fresh,
    Resume(Checkpoint),
}

pub fn plan(request: &Request, map: &SessionMap) -> Plan {
    plan_with_mode(request, map, KeyMode::Strict)
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FreshReason {
    NoKey,
    NoCheckpoint,
    InvalidCheckpoint,
    NotSingleUserTail,
    PrefixMismatch { expected: Hash, actual: Hash },
    PendingMismatch,
    TextWithPending,
}
/// Split before the consecutive user messages appended after an assistant turn.
pub fn user_tail(messages: &[Message]) -> (&[Message], &[Message]) {
    let start = messages
        .iter()
        .rposition(|m| m.role != "user")
        .map_or(0, |i| i + 1);
    messages.split_at(start)
}

pub fn fresh_reason(request: &Request, map: &SessionMap, mode: KeyMode) -> Option<FreshReason> {
    let Some(key) = key_with_mode(request, mode) else {
        return Some(FreshReason::NoKey);
    };
    let Some(cp) = map.get(&key) else {
        return Some(FreshReason::NoCheckpoint);
    };
    if cp.resume_uuid.trim().is_empty() || cp.session_id.trim().is_empty() {
        return Some(FreshReason::InvalidCheckpoint);
    }
    let (prefix, tail) = user_tail(&request.messages);
    if tail.is_empty() {
        return Some(FreshReason::NotSingleUserTail);
    }
    let actual = prefix_hash(prefix);
    if actual != cp.prefix_hash {
        return Some(FreshReason::PrefixMismatch {
            expected: cp.prefix_hash,
            actual,
        });
    }
    if !cp.pending_tool_ids.is_empty() && tail.iter().all(|m| matches!(m.content, Content::Text(_)))
    {
        return Some(FreshReason::TextWithPending);
    }
    if matches!(plan_checked(request, map, mode), Plan::Fresh) {
        Some(FreshReason::PendingMismatch)
    } else {
        None
    }
}
pub fn plan_with_mode(request: &Request, map: &SessionMap, mode: KeyMode) -> Plan {
    plan_checked(request, map, mode)
}
fn plan_checked(request: &Request, map: &SessionMap, mode: KeyMode) -> Plan {
    let Some(key) = key_with_mode(request, mode) else {
        return Plan::Fresh;
    };
    let Some(cp) = map.get(&key) else {
        return Plan::Fresh;
    };
    let (prefix, tail) = user_tail(&request.messages);
    if cp.resume_uuid.trim().is_empty()
        || cp.session_id.trim().is_empty()
        || tail.is_empty()
        || prefix_hash(prefix) != cp.prefix_hash
    {
        return Plan::Fresh;
    }
    let pending: BTreeSet<&str> = cp.pending_tool_ids.iter().map(String::as_str).collect();
    if pending.len() != cp.pending_tool_ids.len() {
        return Plan::Fresh;
    }
    let mut seen = BTreeSet::new();
    let mut after_results = false;
    for block in tail.iter().flat_map(|m| match &m.content {
        Content::Text(text) => alloc::vec![json!({"type":"text","text":text})],
        Content::Blocks(blocks) => blocks.clone(),
    }) {
        match block.get("type").and_then(Value::as_str) {
            Some("tool_result") => {
                let Some(id) = block.get("tool_use_id").and_then(Value::as_str) else {
                    return Plan::Fresh;
                };
                if after_results || !pending.contains(id) || !seen.insert(String::from(id)) {
                    return Plan::Fresh;
                }
            }
            Some("text") => {
                if seen.len() != pending.len() {
                    return Plan::Fresh;
                }
                after_results = true;
            }
            _ if !pending.is_empty() => return Plan::Fresh,
            _ => (),
        }
    }
    if seen.len() != pending.len() {
        return Plan::Fresh;
    }
    Plan::Resume(cp.clone())
}

#[derive(Clone, Debug)]
pub struct SessionMap {
    cap: usize,
    entries: BTreeMap<SessionKey, Checkpoint>,
    lru: Vec<SessionKey>,
}
impl Default for SessionMap {
    fn default() -> Self {
        Self::new(256)
    }
}
impl SessionMap {
    pub fn new(cap: usize) -> Self {
        Self {
            cap,
            entries: BTreeMap::new(),
            lru: Vec::new(),
        }
    }
    /// Read-only: planning never changes map state. The shell can touch on lock acquisition.
    pub fn get(&self, key: &SessionKey) -> Option<&Checkpoint> {
        self.entries.get(key)
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub fn touch(&mut self, key: &SessionKey) {
        if self.entries.contains_key(key) {
            self.lru.retain(|item| item != key);
            self.lru.push(*key);
        }
    }
    pub fn insert(&mut self, key: SessionKey, checkpoint: Checkpoint) {
        if self.cap == 0 {
            return;
        }
        self.entries.insert(key, checkpoint);
        self.touch(&key);
        while self.entries.len() > self.cap {
            let oldest = self.lru.remove(0);
            self.entries.remove(&oldest);
        }
    }
    pub fn remove(&mut self, key: &SessionKey) {
        self.entries.remove(key);
        self.lru.retain(|item| item != key);
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MapEffect {
    Commit(Checkpoint),
    Keep,
    Discard,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Final {
    pub key: Option<SessionKey>,
    pub map_effect: MapEffect,
}
pub fn apply(map: &mut SessionMap, final_state: Final) {
    if let Some(key) = final_state.key {
        match final_state.map_effect {
            MapEffect::Commit(cp) => map.insert(key, cp),
            MapEffect::Keep => (),
            MapEffect::Discard => map.remove(&key),
        }
    }
}

#[cfg(test)]
mod tests;
