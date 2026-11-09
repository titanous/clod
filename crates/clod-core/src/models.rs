//! Pure model discovery from the Claude CLI initialize control response.
//!
//! The catalog comes from the `models` array the CLI returns for an
//! `initialize` control request (`[{value, displayName, description, ...}]`);
//! clod never ships a static model list. The shell spawns [`discovery_spec`],
//! writes [`initialize_line`], feeds stdout lines to [`parse_catalog`] until it
//! yields a catalog or [`DISCOVERY_TIMEOUT_MS`] elapses, then stops the child.
//! No user message is ever written, so no generation is requested.
use crate::{
    protocol,
    request::{Content, Request},
    session::Plan,
    spawn::{self, SpawnSpec},
};
use alloc::{collections::BTreeSet, string::String, vec::Vec};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Control request id used for the discovery initialize request.
pub const INITIALIZE_REQUEST_ID: &str = "clod-models";
/// Upper bound on waiting for the initialize response.
pub const DISCOVERY_TIMEOUT_MS: u64 = 15_000;
/// How long a discovered catalog may be served before rediscovery.
pub const CACHE_TTL_MS: u64 = 300_000;
/// Briefly share failed probes so queued callers do not each start a CLI.
pub const FAILURE_TTL_MS: u64 = 5_000;
pub fn result_fresh(age_ms: u64, success: bool) -> bool {
    age_ms
        < if success {
            CACHE_TTL_MS
        } else {
            FAILURE_TTL_MS
        }
}
/// Default and bounds of the `limit` list parameter.
pub const DEFAULT_LIMIT: usize = 20;
pub const MAX_LIMIT: usize = 1_000;
/// Model alias passed to `--model` for the discovery process; it only has to
/// be accepted by the CLI, since no turn is ever started.
const DISCOVERY_MODEL: &str = "default";

/// One catalog entry in Anthropic Models API shape. `created_at` is not
/// reported by the CLI and is therefore omitted rather than fabricated.
///
/// The optional fields copy typed metadata the CLI reported for the model
/// (`description`, `resolvedModel`, `supportsEffort`, `supportedEffortLevels`,
/// `supportsAdaptiveThinking`, `supportsThinkingOff`, `supportsAutoMode`,
/// `supportsFastMode`). `None` means the CLI did not report the key and is
/// omitted when serialized; `Some(false)` means the CLI reported `false`.
/// Nothing is inferred, and no other initialize data is passed through.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Model {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_effort: Option<bool>,
    /// Effort level names in CLI order. Unknown names are kept as reported so
    /// newer CLI levels remain visible.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supported_effort_levels: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_adaptive_thinking: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_thinking_off: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_auto_mode: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_fast_mode: Option<bool>,
}

impl Model {
    /// A catalog entry with only the required fields.
    pub fn new(id: impl Into<String>, display_name: impl Into<String>) -> Self {
        Model {
            id: id.into(),
            kind: "model".into(),
            display_name: display_name.into(),
            description: None,
            resolved_model: None,
            supports_effort: None,
            supported_effort_levels: None,
            supports_adaptive_thinking: None,
            supports_thinking_off: None,
            supports_auto_mode: None,
            supports_fast_mode: None,
        }
    }
}

/// Query parameters of `GET /v1/models`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct ListQuery {
    pub before_id: Option<String>,
    pub after_id: Option<String>,
    pub limit: Option<usize>,
}

/// Spawn specification for a discovery process. Reuses [`spawn::spec`] with a
/// synthetic request that has no messages, no tools, no system prompt and no
/// thinking/effort options, so argv/env/working directory match ordinary
/// turns (setting sources, MCP strictness, removed credentials env, upstream
/// recorder) without any generation input.
pub fn discovery_spec(config: &spawn::Config) -> SpawnSpec {
    let request = Request {
        model: DISCOVERY_MODEL.into(),
        system: Content::default(),
        messages: Vec::new(),
        tools: Vec::new(),
        max_tokens: 1,
        thinking: None,
        output_config: None,
        stream: false,
        conversation_id: None,
    };
    spawn::spec(&request, &Plan::Fresh, config)
}

/// Newline-terminated initialize control request with no hooks, MCP servers
/// or system prompt.
pub fn initialize_line() -> String {
    let mut line = protocol::control(INITIALIZE_REQUEST_ID, json!({"subtype":"initialize"}));
    line.push('\n');
    line
}

/// Parse one CLI stdout line. `Ok(None)` for lines that are not the
/// discovery initialize response; `Ok(Some(catalog))` for a valid success
/// response; `Err` for a matching response that failed or is malformed.
pub fn parse_catalog(line: &str) -> Result<Option<Vec<Model>>, &'static str> {
    let Ok(value) = serde_json::from_str::<Value>(line.trim()) else {
        return Ok(None);
    };
    if value["type"] != "control_response" {
        return Ok(None);
    }
    let response = &value["response"];
    if response["request_id"] != INITIALIZE_REQUEST_ID {
        return Ok(None);
    }
    if response["subtype"] != "success" {
        return Err("initialize control request failed");
    }
    let models = response["response"]["models"]
        .as_array()
        .ok_or("initialize response has no models array")?;
    if models.is_empty() {
        return Err("initialize response has an empty models array");
    }
    let mut seen = BTreeSet::new();
    let mut catalog = Vec::with_capacity(models.len());
    for entry in models {
        let id = nonempty(&entry["value"]).ok_or("model value must be a non-empty string")?;
        let display_name = nonempty(&entry["displayName"])
            .ok_or("model displayName must be a non-empty string")?;
        let description = optional_string(entry, "description", false)
            .ok_or("model description must be a string")?;
        let resolved_model = optional_string(entry, "resolvedModel", true)
            .ok_or("model resolvedModel must be a non-empty string")?;
        let supported_effort_levels = effort_levels(entry)
            .ok_or("model supportedEffortLevels must be an array of unique non-empty strings")?;
        let flag = |key, message| optional_bool(entry, key).ok_or(message);
        let supports_effort = flag("supportsEffort", "model supportsEffort must be a boolean")?;
        let supports_adaptive_thinking = flag(
            "supportsAdaptiveThinking",
            "model supportsAdaptiveThinking must be a boolean",
        )?;
        let supports_thinking_off = flag(
            "supportsThinkingOff",
            "model supportsThinkingOff must be a boolean",
        )?;
        let supports_auto_mode = flag(
            "supportsAutoMode",
            "model supportsAutoMode must be a boolean",
        )?;
        let supports_fast_mode = flag(
            "supportsFastMode",
            "model supportsFastMode must be a boolean",
        )?;
        if !seen.insert(id) {
            return Err("duplicate model value in initialize response");
        }
        catalog.push(Model {
            description,
            resolved_model,
            supports_effort,
            supported_effort_levels,
            supports_adaptive_thinking,
            supports_thinking_off,
            supports_auto_mode,
            supports_fast_mode,
            ..Model::new(id, display_name)
        });
    }
    // Keep CLI selections in order; additionally advertise resolved IDs for
    // clients whose model picker consumes IDs but ignores display metadata.
    // Explicit CLI entries win over metadata copied from an alias.
    let mut ids: BTreeSet<String> = catalog.iter().map(|model| model.id.clone()).collect();
    let mut resolved = Vec::new();
    for model in &catalog {
        if let Some(id) = &model.resolved_model {
            if ids.insert(id.clone()) {
                let mut entry = model.clone();
                entry.id = id.clone();
                resolved.push(entry);
            }
        }
    }
    catalog.extend(resolved);
    Ok(Some(catalog))
}

fn nonempty(value: &Value) -> Option<&str> {
    value.as_str().filter(|s| !s.trim().is_empty())
}

/// `Some(None)` when `key` is absent, `Some(Some(_))` for a valid string and
/// `None` for a present but invalid value. JSON `null` counts as invalid.
fn optional_string(entry: &Value, key: &str, require_nonempty: bool) -> Option<Option<String>> {
    match entry.get(key) {
        None => Some(None),
        Some(value) if require_nonempty => nonempty(value).map(|s| Some(s.into())),
        Some(value) => value.as_str().map(|s| Some(s.into())),
    }
}

fn optional_bool(entry: &Value, key: &str) -> Option<Option<bool>> {
    match entry.get(key) {
        None => Some(None),
        Some(value) => value.as_bool().map(Some),
    }
}

/// Effort level names are kept verbatim, including names clod does not know,
/// but each must be a non-empty string and appear once.
fn effort_levels(entry: &Value) -> Option<Option<Vec<String>>> {
    let Some(value) = entry.get("supportedEffortLevels") else {
        return Some(None);
    };
    let mut seen = BTreeSet::new();
    let mut levels = Vec::new();
    for level in value.as_array()? {
        let level = nonempty(level)?;
        if !seen.insert(level) {
            return None;
        }
        levels.push(level.into());
    }
    Some(Some(levels))
}

/// Find a catalog entry by id.
pub fn find<'a>(catalog: &'a [Model], id: &str) -> Option<&'a Model> {
    catalog.iter().find(|m| m.id == id)
}

/// Whether a catalog discovered `age_ms` ago may still be served.
pub fn cache_fresh(age_ms: u64) -> bool {
    age_ms < CACHE_TTL_MS
}

/// Catalog-independent query checks (limit range, conflicting cursors), so a
/// bad request is rejected before any discovery process is spawned. Whether
/// a cursor names a known model is checked by [`page`].
pub fn validate_query(query: &ListQuery) -> Result<(), &'static str> {
    if query.before_id.is_some() && query.after_id.is_some() {
        return Err("before_id and after_id are mutually exclusive");
    }
    if !(1..=MAX_LIMIT).contains(&query.limit.unwrap_or(DEFAULT_LIMIT)) {
        return Err("limit must be between 1 and 1000");
    }
    Ok(())
}

/// One page of the catalog in CLI order, as a Models API list body:
/// `{data, has_more, first_id, last_id}`. `after_id` returns entries after
/// the cursor, `before_id` the entries immediately before it.
pub fn page(catalog: &[Model], query: &ListQuery) -> Result<Value, &'static str> {
    validate_query(query)?;
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT);
    let position = |id: &str| {
        catalog
            .iter()
            .position(|m| m.id == id)
            .ok_or("unknown model id cursor")
    };
    let (start, end, has_more) = match (&query.before_id, &query.after_id) {
        (Some(_), Some(_)) => unreachable!("rejected by validate_query"),
        (Some(before), None) => {
            let end = position(before)?;
            let start = end.saturating_sub(limit);
            (start, end, start > 0)
        }
        (None, Some(after)) => {
            let start = position(after)? + 1;
            let end = (start + limit).min(catalog.len());
            (start, end, end < catalog.len())
        }
        (None, None) => {
            let end = limit.min(catalog.len());
            (0, end, end < catalog.len())
        }
    };
    let data = &catalog[start..end];
    Ok(json!({
        "data": data,
        "has_more": has_more,
        "first_id": data.first().map(|m| m.id.as_str()),
        "last_id": data.last().map(|m| m.id.as_str()),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::{format, string::ToString};

    fn success(models: Value) -> String {
        json!({"type":"control_response","response":{"subtype":"success","request_id":INITIALIZE_REQUEST_ID,"response":{"commands":[],"models":models},"pending_permission_requests":[]}}).to_string()
    }
    fn entry(value: &str, display: &str) -> Value {
        json!({"value":value,"resolvedModel":"x","displayName":display,"description":"d","supportsEffort":true})
    }
    fn catalog(n: usize) -> Vec<Model> {
        (0..n)
            .map(|i| Model::new(format!("m{i}"), format!("M{i}")))
            .collect()
    }
    fn ids(page: &Value) -> Vec<&str> {
        page["data"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["id"].as_str().unwrap())
            .collect()
    }
    fn q(before: Option<&str>, after: Option<&str>, limit: Option<usize>) -> ListQuery {
        ListQuery {
            before_id: before.map(Into::into),
            after_id: after.map(Into::into),
            limit,
        }
    }

    #[test]
    fn constants() {
        assert_eq!(INITIALIZE_REQUEST_ID, "clod-models");
        assert_eq!(DISCOVERY_TIMEOUT_MS, 15_000);
        assert_eq!(CACHE_TTL_MS, 300_000);
    }

    #[test]
    fn initialize_line_shape() {
        let line = initialize_line();
        assert!(line.ends_with('\n'));
        assert_eq!(line.matches('\n').count(), 1);
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(
            v,
            json!({"type":"control_request","request_id":"clod-models","request":{"subtype":"initialize"}})
        );
        assert!(!line.contains("\"user\""));
    }

    #[test]
    fn discovery_spec_matches_turn_env_without_generation_input() {
        let mut config = spawn::Config::new("/tmp/clod-empty");
        config.is_root = true;
        config.upstream_base_url = Some("http://127.0.0.1:9/".into());
        let spec = discovery_spec(&config);
        assert_eq!(spec.program, "claude");
        assert_eq!(spec.working_directory, "/tmp/clod-empty");
        for flag in [
            "--setting-sources=",
            "--strict-mcp-config",
            "--tools",
            "--input-format",
        ] {
            assert!(spec.args.iter().any(|a| a == flag), "{flag}");
        }
        for flag in [
            "--resume",
            "--fork-session",
            "--max-thinking-tokens",
            "--effort",
        ] {
            assert!(!spec.args.iter().any(|a| a == flag), "{flag}");
        }
        let allowed = spec
            .args
            .iter()
            .position(|a| a == "--allowedTools")
            .unwrap();
        assert_eq!(spec.args[allowed + 1], "");
        assert!(spec.env_set.contains(&("IS_SANDBOX".into(), "1".into())));
        assert!(spec
            .env_set
            .contains(&("ANTHROPIC_BASE_URL".into(), "http://127.0.0.1:9/".into())));
        assert!(spec.env_remove.iter().any(|e| e == "CLAUDE_CONFIG_DIR"));
        assert!(spec.env_remove.iter().any(|e| e == "ANTHROPIC_API_KEY"));
        let plain = discovery_spec(&spawn::Config::new("/w"));
        assert!(plain.env_remove.iter().any(|e| e == "ANTHROPIC_BASE_URL"));
    }

    #[test]
    fn resolved_ids_are_appended_once_and_explicit_entries_win() {
        let catalog = parse_catalog(&success(json!([
            {"value":"default","displayName":"Default","resolvedModel":"claude-fable-5-2","supportsEffort":true},
            {"value":"fable","displayName":"Fable 5.2","resolvedModel":"claude-fable-5-2"},
            {"value":"opus","displayName":"Alias","resolvedModel":"claude-opus-5-5"},
            {"value":"claude-opus-5-5","displayName":"Explicit","supportsThinkingOff":false}
        ]))).unwrap().unwrap();
        assert_eq!(
            catalog.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            [
                "default",
                "fable",
                "opus",
                "claude-opus-5-5",
                "claude-fable-5-2"
            ]
        );
        assert_eq!(
            find(&catalog, "claude-fable-5-2").unwrap().supports_effort,
            Some(true)
        );
        assert_eq!(
            find(&catalog, "claude-opus-5-5").unwrap().display_name,
            "Explicit"
        );
        assert_eq!(
            find(&catalog, "claude-opus-5-5")
                .unwrap()
                .supports_thinking_off,
            Some(false)
        );
    }

    #[test]
    fn parses_fixture_catalog_in_order() {
        let fixture = include_str!("../tests/fixtures/a-text.ndjson");
        let mut found = None;
        for wrapped in fixture.lines() {
            let v: Value = serde_json::from_str(wrapped).unwrap();
            let Some(mut line) = v.get("line").cloned() else {
                continue;
            };
            if line["type"] == "control_response"
                && line["response"]["response"].get("models").is_some()
            {
                line["response"]["request_id"] = INITIALIZE_REQUEST_ID.into();
                found = parse_catalog(&line.to_string()).unwrap();
                break;
            }
        }
        let catalog = found.expect("fixture has an initialize models response");
        assert_eq!(catalog[0].id, "default");
        assert_eq!(catalog[0].display_name, "Default (recommended)");
        assert!(catalog.iter().any(|m| m.id == "opus"));
        assert!(catalog.iter().all(|m| m.kind == "model"));
        let fable = find(&catalog, "claude-fable-5").expect("fixture fable entry");
        assert_eq!(fable.resolved_model.as_deref(), Some("claude-fable-5"));
        assert_eq!(fable.supports_thinking_off, Some(false));
        assert_eq!(fable.supports_fast_mode, None);
        assert_eq!(fable.supports_effort, Some(true));
        let mythos = find(&catalog, "claude-mythos-5-1").expect("fixture mythos entry");
        assert_eq!(mythos.description.as_deref(), Some(""));
    }

    #[test]
    fn parses_success_and_serializes_without_created_at() {
        let line = success(json!([entry("opus", "Opus"), {"value":"haiku","displayName":"Haiku"}]));
        let catalog = parse_catalog(&format!("{line}\n")).unwrap().unwrap();
        assert_eq!(catalog.len(), 3);
        assert_eq!(catalog[2].id, "x");
        let v = serde_json::to_value(&catalog[0]).unwrap();
        assert_eq!(
            v,
            json!({"id":"opus","type":"model","display_name":"Opus","description":"d","resolved_model":"x","supports_effort":true})
        );
        let back: Model = serde_json::from_value(v).unwrap();
        assert_eq!(back, catalog[0]);
        let bare = serde_json::to_value(&catalog[1]).unwrap();
        assert_eq!(
            bare,
            json!({"id":"haiku","type":"model","display_name":"Haiku"})
        );
        assert_eq!(catalog[1], Model::new("haiku", "Haiku"));
        let back: Model = serde_json::from_value(bare).unwrap();
        assert_eq!(back, catalog[1]);
    }

    fn one(entry: Value) -> Result<Model, &'static str> {
        parse_catalog(&success(json!([entry]))).map(|c| c.unwrap().remove(0))
    }

    #[test]
    fn maps_all_metadata_to_snake_case() {
        let model = one(json!({
            "value":"opus","displayName":"Opus","description":"Larger model",
            "resolvedModel":"claude-opus-5-5","supportsEffort":true,
            "supportedEffortLevels":["low","medium","high","xhigh","max"],
            "supportsAdaptiveThinking":true,"supportsThinkingOff":false,
            "supportsAutoMode":true,"supportsFastMode":true
        }))
        .unwrap();
        assert_eq!(model.description.as_deref(), Some("Larger model"));
        assert_eq!(model.resolved_model.as_deref(), Some("claude-opus-5-5"));
        assert_eq!(model.supports_effort, Some(true));
        assert_eq!(
            model.supported_effort_levels.as_deref(),
            Some(&["low", "medium", "high", "xhigh", "max"].map(String::from)[..])
        );
        assert_eq!(model.supports_adaptive_thinking, Some(true));
        assert_eq!(model.supports_thinking_off, Some(false));
        assert_eq!(model.supports_auto_mode, Some(true));
        assert_eq!(model.supports_fast_mode, Some(true));
        let v = serde_json::to_value(&model).unwrap();
        assert_eq!(
            v,
            json!({
                "id":"opus","type":"model","display_name":"Opus","description":"Larger model",
                "resolved_model":"claude-opus-5-5","supports_effort":true,
                "supported_effort_levels":["low","medium","high","xhigh","max"],
                "supports_adaptive_thinking":true,"supports_thinking_off":false,
                "supports_auto_mode":true,"supports_fast_mode":true
            })
        );
        let text = v.to_string();
        for camel in [
            "displayName",
            "resolvedModel",
            "supportsEffort",
            "supportedEffortLevels",
        ] {
            assert!(!text.contains(camel), "{camel}");
        }
        let back: Model = serde_json::from_value(v).unwrap();
        assert_eq!(back, model);
    }

    #[test]
    fn missing_metadata_is_omitted_and_false_is_kept() {
        let missing = one(json!({"value":"m","displayName":"M"})).unwrap();
        assert_eq!(missing, Model::new("m", "M"));
        let v = serde_json::to_value(&missing).unwrap();
        assert_eq!(v.as_object().unwrap().len(), 3);
        let falsy = one(json!({
            "value":"m","displayName":"M","description":"","supportsEffort":false,
            "supportedEffortLevels":[],"supportsAdaptiveThinking":false,
            "supportsThinkingOff":false,"supportsAutoMode":false,"supportsFastMode":false
        }))
        .unwrap();
        assert_eq!(
            serde_json::to_value(&falsy).unwrap(),
            json!({
                "id":"m","type":"model","display_name":"M","description":"","supports_effort":false,
                "supported_effort_levels":[],"supports_adaptive_thinking":false,
                "supports_thinking_off":false,"supports_auto_mode":false,"supports_fast_mode":false
            })
        );
        // Each flag independently: present-false differs from absent.
        let only_fast =
            one(json!({"value":"m","displayName":"M","supportsFastMode":false})).unwrap();
        assert_eq!(only_fast.supports_fast_mode, Some(false));
        assert_eq!(only_fast.supports_auto_mode, None);
        assert_ne!(only_fast, missing);
        // Deserializing a body without metadata keys yields None, not false.
        let parsed: Model =
            serde_json::from_str(r#"{"id":"m","type":"model","display_name":"M"}"#).unwrap();
        assert_eq!(parsed, missing);
    }

    #[test]
    fn unknown_effort_levels_are_forward_compatible() {
        let model = one(
            json!({"value":"m","displayName":"M","supportedEffortLevels":["low","ultra-9000"]}),
        )
        .unwrap();
        assert_eq!(
            model.supported_effort_levels,
            Some(alloc::vec!["low".into(), "ultra-9000".into()])
        );
    }

    #[test]
    fn rejects_malformed_metadata() {
        let base = |key: &str, value: Value| {
            let mut e = json!({"value":"m","displayName":"M"});
            e[key] = value;
            e
        };
        let bools = [
            "supportsEffort",
            "supportsAdaptiveThinking",
            "supportsThinkingOff",
            "supportsAutoMode",
            "supportsFastMode",
        ];
        for key in bools {
            for bad in [
                json!("true"),
                json!(1),
                json!(null),
                json!([true]),
                json!({}),
            ] {
                assert!(one(base(key, bad.clone())).is_err(), "{key} {bad}");
            }
        }
        for bad in [json!(3), json!(null), json!(["d"]), json!(false)] {
            assert!(one(base("description", bad.clone())).is_err(), "{bad}");
        }
        for bad in [json!(""), json!("  "), json!(null), json!(1), json!({})] {
            assert!(one(base("resolvedModel", bad.clone())).is_err(), "{bad}");
        }
        for bad in [
            json!("low"),
            json!(null),
            json!({"low":true}),
            json!([1]),
            json!([null]),
            json!([""]),
            json!(["low", ["high"]]),
            json!(["low", "low"]),
        ] {
            assert!(
                one(base("supportedEffortLevels", bad.clone())).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn duplicates_are_rejected_or_resolved_safely() {
        // Duplicate model values are still rejected even with metadata.
        let dup = json!([
            {"value":"m","displayName":"M","supportsFastMode":true},
            {"value":"m","displayName":"M2","supportsFastMode":false}
        ]);
        assert!(parse_catalog(&success(dup)).is_err());
        // Duplicate JSON keys resolve to one value before validation, so a
        // malformed duplicate cannot slip past type checks.
        let prefix = format!(
            r#"{{"type":"control_response","response":{{"subtype":"success","request_id":"{INITIALIZE_REQUEST_ID}","response":{{"models":["#
        );
        let line = |entry: &str| format!("{prefix}{entry}]}}}}}}");
        let ok = parse_catalog(&line(
            r#"{"value":"m","displayName":"M","supportsFastMode":"yes","supportsFastMode":true}"#,
        ))
        .unwrap()
        .unwrap();
        assert_eq!(ok[0].supports_fast_mode, Some(true));
        assert!(parse_catalog(&line(
            r#"{"value":"m","displayName":"M","supportsFastMode":true,"supportsFastMode":"yes"}"#
        ))
        .is_err());
    }

    #[test]
    fn unrelated_initialize_data_is_not_passed_through() {
        let model = one(json!({
            "value":"m","displayName":"M","supportsFastMode":true,
            "account":{"email":"private@example.com"},"contextWindow":1000000,
            "maxOutputTokens":64000,"pricing":{"input":1},"supportsVision":true
        }))
        .unwrap();
        let text = serde_json::to_string(&model).unwrap();
        assert_eq!(
            text,
            r#"{"id":"m","type":"model","display_name":"M","supports_fast_mode":true}"#
        );
        // Unknown body keys are also ignored when deserializing a Model.
        let parsed: Model = serde_json::from_str(
            r#"{"id":"m","type":"model","display_name":"M","account":{"x":1}}"#,
        )
        .unwrap();
        assert_eq!(parsed, Model::new("m", "M"));
    }

    #[test]
    fn skips_unrelated_lines() {
        for line in [
            "",
            "not json",
            "[1,2]",
            r#"{"type":"system","subtype":"init"}"#,
            r#"{"type":"control_request","request_id":"clod-models","request":{"subtype":"initialize"}}"#,
            r#"{"type":"control_response","response":{"subtype":"success","request_id":"other","response":{"models":[]}}}"#,
            r#"{"type":"control_response","response":{"subtype":"error","request_id":"other"}}"#,
        ] {
            assert_eq!(parse_catalog(line), Ok(None), "{line}");
        }
    }

    #[test]
    fn rejects_failed_or_malformed_matching_responses() {
        let error = json!({"type":"control_response","response":{"subtype":"error","request_id":"clod-models","error":"x"}}).to_string();
        assert!(parse_catalog(&error).is_err());
        let no_models = json!({"type":"control_response","response":{"subtype":"success","request_id":"clod-models","response":{}}}).to_string();
        assert!(parse_catalog(&no_models).is_err());
        let not_array = success(json!({"value":"opus"}));
        assert!(parse_catalog(&not_array).is_err());
        let bad = [
            json!([]),
            json!([entry("opus", "Opus"), entry("opus", "Again")]),
            json!([{"displayName":"x"}]),
            json!([{"value":"","displayName":"x"}]),
            json!([{"value":"  ","displayName":"x"}]),
            json!([{"value":7,"displayName":"x"}]),
            json!([{"value":"opus"}]),
            json!([{"value":"opus","displayName":""}]),
            json!([{"value":"opus","displayName":"Opus","description":3}]),
            json!(["opus"]),
        ];
        for models in bad {
            assert!(parse_catalog(&success(models.clone())).is_err(), "{models}");
        }
    }

    #[test]
    fn list_query_deserializes_defaults() {
        let q: ListQuery = serde_json::from_str("{}").unwrap();
        assert_eq!(q, ListQuery::default());
        let q: ListQuery = serde_json::from_str(r#"{"after_id":"a","limit":5}"#).unwrap();
        assert_eq!(q.after_id.as_deref(), Some("a"));
        assert_eq!(q.limit, Some(5));
    }

    #[test]
    fn default_page() {
        let c = catalog(25);
        let p = page(&c, &ListQuery::default()).unwrap();
        assert_eq!(ids(&p).len(), 20);
        assert_eq!(p["has_more"], true);
        assert_eq!(p["first_id"], "m0");
        assert_eq!(p["last_id"], "m19");
        assert!(p["data"][0].get("created_at").is_none());
        let small = page(&catalog(3), &ListQuery::default()).unwrap();
        assert_eq!(ids(&small), ["m0", "m1", "m2"]);
        assert_eq!(small["has_more"], false);
    }

    #[test]
    fn limit_bounds() {
        let c = catalog(3);
        assert!(page(&c, &q(None, None, Some(0))).is_err());
        assert!(page(&c, &q(None, None, Some(1001))).is_err());
        assert!(page(&c, &q(None, None, Some(1000))).is_ok());
        let p = page(&c, &q(None, None, Some(1))).unwrap();
        assert_eq!(ids(&p), ["m0"]);
        assert_eq!(p["has_more"], true);
    }

    #[test]
    fn after_cursor_walks_forward() {
        let c = catalog(5);
        let p = page(&c, &q(None, Some("m1"), Some(2))).unwrap();
        assert_eq!(ids(&p), ["m2", "m3"]);
        assert_eq!(p["has_more"], true);
        let p = page(&c, &q(None, Some("m3"), Some(2))).unwrap();
        assert_eq!(ids(&p), ["m4"]);
        assert_eq!(p["has_more"], false);
        let p = page(&c, &q(None, Some("m4"), None)).unwrap();
        assert!(ids(&p).is_empty());
        assert_eq!(p["has_more"], false);
        assert_eq!(p["first_id"], Value::Null);
        assert_eq!(p["last_id"], Value::Null);
    }

    #[test]
    fn before_cursor_walks_backward() {
        let c = catalog(5);
        let p = page(&c, &q(Some("m4"), None, Some(2))).unwrap();
        assert_eq!(ids(&p), ["m2", "m3"]);
        assert_eq!(p["has_more"], true);
        let p = page(&c, &q(Some("m2"), None, Some(5))).unwrap();
        assert_eq!(ids(&p), ["m0", "m1"]);
        assert_eq!(p["has_more"], false);
        let p = page(&c, &q(Some("m0"), None, None)).unwrap();
        assert!(ids(&p).is_empty());
        assert_eq!(p["has_more"], false);
    }

    #[test]
    fn invalid_cursors() {
        let c = catalog(3);
        assert!(page(&c, &q(Some("m0"), Some("m1"), None)).is_err());
        assert!(page(&c, &q(Some("nope"), None, None)).is_err());
        assert!(page(&c, &q(None, Some("nope"), None)).is_err());
        assert!(page(&[], &q(None, Some("m0"), None)).is_err());
        let empty = page(&[], &ListQuery::default()).unwrap();
        assert_eq!(empty["has_more"], false);
        assert_eq!(empty["data"], json!([]));
    }

    #[test]
    fn cache_freshness_boundary() {
        assert!(cache_fresh(0));
        assert!(cache_fresh(CACHE_TTL_MS - 1));
        assert!(!cache_fresh(CACHE_TTL_MS));
        assert!(!cache_fresh(u64::MAX));
    }

    #[test]
    fn validate_query_without_catalog() {
        assert_eq!(validate_query(&ListQuery::default()), Ok(()));
        assert_eq!(
            validate_query(&q(Some("unknown"), None, Some(1000))),
            Ok(())
        );
        assert_eq!(validate_query(&q(None, Some("unknown"), Some(1))), Ok(()));
        assert!(validate_query(&q(Some("a"), Some("b"), None)).is_err());
        assert!(validate_query(&q(None, None, Some(0))).is_err());
        assert!(validate_query(&q(None, None, Some(1001))).is_err());
    }

    #[test]
    fn lookup() {
        let c = catalog(3);
        assert_eq!(find(&c, "m1").map(|m| m.display_name.as_str()), Some("M1"));
        assert!(find(&c, "zz").is_none());
    }
}
