//! Pure process specification; the shell validates/creates the fixed empty directory.
use crate::{
    replay,
    request::{Content, Request, TOOL_PREFIX},
    session::Plan,
};
use alloc::{
    format,
    string::{String, ToString},
    vec,
    vec::Vec,
};
use serde_json::Value;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpawnSpec {
    pub program: String,
    pub args: Vec<String>,
    pub env_set: Vec<(String, String)>,
    pub env_remove: Vec<String>,
    pub working_directory: String,
}
#[derive(Clone, Debug)]
pub struct Config {
    pub program: String,
    /// Experimental transport name instructions and Fresh replay presentation.
    /// Keep fixed for the lifetime of a process/session store.
    pub align_tool_names: bool,
    /// Fixed, empty, non-git directory. Never derived from request content.
    pub working_directory: String,
    pub is_root: bool,
    pub session_key: crate::session::KeyMode,
    /// Measurement-only upstream recorder URL; never inferred from a request.
    pub upstream_base_url: Option<String>,
    pub ping_ms: u64,
    pub hold_idle_ms: u64,
    pub turn_timeout_ms: u64,
}
impl Config {
    pub fn new(working_directory: impl Into<String>) -> Self {
        Self {
            program: "claude".into(),
            align_tool_names: false,
            working_directory: working_directory.into(),
            is_root: false,
            session_key: crate::session::KeyMode::Strict,
            upstream_base_url: None,
            ping_ms: 15_000,
            hold_idle_ms: 60_000,
            turn_timeout_ms: 900_000,
        }
    }
}
pub fn system_prompt(request: &Request, plan: &Plan) -> String {
    let mut text = match &request.system {
        Content::Text(s) => s.clone(),
        Content::Blocks(blocks) => blocks
            .iter()
            .filter_map(|b| b["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
    };
    if matches!(plan, Plan::Fresh) {
        text.push_str(replay::PROVENANCE_NOTE);
    }
    text
}
pub fn configured_system_prompt(request: &Request, plan: &Plan, config: &Config) -> String {
    let mut text = system_prompt(request, plan);
    if config.align_tool_names {
        let registry = crate::name_alignment::Registry::new(&request.tools);
        // Rewrite the incoming system only, before adding clod's own notes.
        let mut transport = request.clone();
        transport.system = match &request.system {
            Content::Text(s) => Content::Text(registry.rewrite_instructions(s)),
            Content::Blocks(blocks) => Content::Blocks(
                blocks
                    .iter()
                    .map(|block| {
                        let mut block = block.clone();
                        if block["type"] == "text" {
                            if let Some(text) = block["text"].as_str() {
                                block["text"] = registry.rewrite_instructions(text).into();
                            }
                        }
                        block
                    })
                    .collect(),
            ),
        };
        text = system_prompt(&transport, plan);
        text.push_str(&registry.instruction());
    }
    text
}
pub fn spec(request: &Request, plan: &Plan, config: &Config) -> SpawnSpec {
    let mut args = vec![
        "--output-format".into(),
        "stream-json".into(),
        "--verbose".into(),
        "--input-format".into(),
        "stream-json".into(),
        "--max-turns".into(),
        "1".into(),
        "--model".into(),
        request.model.clone(),
    ];
    if let Plan::Resume(cp) = plan {
        args.extend(["--resume".into(), cp.session_id.clone()]);
    }
    let tools = request
        .tools
        .iter()
        .filter_map(|t| t["name"].as_str())
        .map(|n| format!("{TOOL_PREFIX}{n}"))
        .collect::<Vec<_>>()
        .join(",");
    args.extend([
        "--allowedTools".into(),
        tools,
        "--tools".into(),
        "--setting-sources=".into(),
        "--strict-mcp-config".into(),
        "--permission-mode".into(),
        "bypassPermissions".into(),
        "--allow-dangerously-skip-permissions".into(),
        "--include-partial-messages".into(),
    ]);
    if let Plan::Resume(cp) = plan {
        args.extend([
            "--fork-session".into(),
            "--resume-session-at".into(),
            cp.resume_uuid.clone(),
        ]);
    }
    if let Some(thinking) = &request.thinking {
        if thinking["type"] == "disabled" {
            args.extend(["--max-thinking-tokens".into(), "0".into()]);
        } else {
            if let Some(budget) = thinking["budget_tokens"].as_u64() {
                args.extend(["--max-thinking-tokens".into(), budget.to_string()]);
            }
            let display = thinking.get("display").and_then(Value::as_str).or_else(|| {
                if thinking.get("display").is_none() {
                    Some("summarized")
                } else {
                    None
                }
            });
            if let Some(display @ ("summarized" | "omitted" | "highlights")) = display {
                args.extend(["--thinking-display".into(), display.into()]);
            }
        }
    }
    if let Some(effort) = request.output_config.as_ref().and_then(|o| o.get("effort")) {
        if let Some(level) = effort.as_str() {
            args.extend(["--effort".into(), level.into()]);
        }
    }
    let mut env_set = vec![
        ("CLAUDE_CODE_DISABLE_ATTACHMENTS".into(), "1".into()),
        ("ENABLE_CLAUDEAI_MCP_SERVERS".into(), "false".into()),
        ("ENABLE_TOOL_SEARCH".into(), "false".into()),
        (
            "CLAUDE_CODE_MAX_OUTPUT_TOKENS".into(),
            request.max_tokens.to_string(),
        ),
    ];
    if config.is_root {
        env_set.push(("IS_SANDBOX".into(), "1".into()));
    }
    if let Some(url) = &config.upstream_base_url {
        env_set.push(("ANTHROPIC_BASE_URL".into(), url.clone()));
    }
    let mut env_remove: Vec<String> = [
        "CLAUDE_CONFIG_DIR",
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "CLAUDE_CODE_OAUTH_TOKEN",
        "CLAUDECODE",
    ]
    .map(String::from)
    .to_vec();
    if config.upstream_base_url.is_none() {
        env_remove.push("ANTHROPIC_BASE_URL".into());
    }
    SpawnSpec {
        program: config.program.clone(),
        args,
        env_set,
        env_remove,
        working_directory: config.working_directory.clone(),
    }
}
