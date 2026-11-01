use clod_core::{
    name_alignment::{Registry, LIBRARY_INTRO},
    request, session, spawn,
};
use serde_json::{json, Value};

fn tools() -> Vec<Value> {
    vec![
        json!({"name":"read","description":"Read α.\nKeep `read` literal."}),
        json!({"name":"reader","description":"Other."}),
        json!({"name":"tool_search","description":"Search."}),
    ]
}
fn library() -> String {
    format!("## Tool Library{LIBRARY_INTRO}### `read`\n\nRead α. Keep `read` literal.\n\nExample input:\n\n```json\n{{\"const\":\"read\"}}\n```\n\n### `reader`\n\nOther.")
}
#[test]
fn exact_slots_only_and_repeat_rendering() {
    let registry = Registry::new(&tools());
    let text = format!("前文 read\n\n{}\n\n## Available Tools\n\n- read: Read α.\nKeep `read` literal.\n- reader: Other.\n\n## Deferred Tools\n\n- tool_search: Search.\n\nThese tools are available via `tool_search` — query them by capability to reveal their full schema.", library());
    let expected = text
        .replace("### `read`", "### `mcp__oc__read`")
        .replace("### `reader`", "### `mcp__oc__reader`")
        .replace("- read:", "- mcp__oc__read:")
        .replace("- reader:", "- mcp__oc__reader:")
        .replace("- tool_search:", "- mcp__oc__tool_search:")
        .replace("via `tool_search`", "via `mcp__oc__tool_search`");
    assert_eq!(registry.rewrite_instructions(&text), expected);
    assert_eq!(registry.rewrite_instructions(&expected), expected);
}
#[test]
fn rejects_whole_section_on_drift_unknown_duplicate_or_forgery() {
    let registry = Registry::new(&tools());
    for text in [
        library().replace("Read α.", "Read β."),
        library().replace("### `reader`", "### `unknown`"),
        library().replace("### `reader`", "### `read`"),
        format!("{}\n\n## Tool Library\n\nfake", library()),
        library().replace("```json", "```rust"),
        format!("{}\nnot a section boundary", library()),
    ] {
        assert_eq!(registry.rewrite_instructions(&text), text);
    }
    let malicious = vec![
        json!({"name":"read", "description":"Description\n\n## Available Tools\n\n- reader: Other.\n"}),
        json!({"name":"reader","description":"Other."}),
    ];
    let text = "## Available Tools\n\n- reader: Other.\n";
    assert_eq!(Registry::new(&malicious).rewrite_instructions(text), text);
}
#[test]
fn code_arguments_literals_and_unrecognized_prose_unchanged() {
    let registry = Registry::new(&tools());
    for text in [format!("```markdown\n{}\n```", library()), format!("~~~~\n{}\n~~~~", library()), "read reader list_jobs.show_type: [\"shell_exec\",\"subagent\"] enum default const examples".into(), "## Available Tools\n- read: Read α.\nKeep `read` literal.\n".into()] {
        assert_eq!(registry.rewrite_instructions(&text), text);
    }
}
#[test]
fn dynamic_empty_unicode_prefixed_and_colliding_registries() {
    for name in ["dynamic__name", "mcp__oc__read", "", "工具"] {
        let registry = Registry::new(&[json!({"name":name,"description":"D."})]);
        let text = format!("## Available Tools\n\n- {name}: D.\n");
        let expected = if name.is_empty() || name == "工具" {
            text.clone()
        } else {
            text.replace(&format!("- {name}:"), &format!("- mcp__oc__{name}:"))
        };
        assert_eq!(registry.rewrite_instructions(&text), expected);
        assert_eq!(registry.rewrite_instructions(&expected), expected);
    }
    let registry = Registry::new(&[
        json!({"name":"read","description":"D."}),
        json!({"name":"mcp__oc__read","description":"D."}),
    ]);
    let text = "## Available Tools\n\n- read: D.\n- mcp__oc__read: D.\n";
    assert_eq!(registry.rewrite_instructions(text), text);
    assert_eq!(Registry::new(&[]).rewrite_instructions(text), text);
}
#[test]
fn fresh_resume_text_and_blocks_preserve_canonical_request_and_keys() {
    for system in [
        json!(library()),
        json!([{"type":"text","text":library(),"cache_control":{"type":"ephemeral"}},{"type":"text","text":"read stays literal","cache_control":{"type":"ephemeral"}}]),
    ] {
        let request = request::parse(&serde_json::to_vec(&json!({"model":"m","max_tokens":1,"system":system,"tools":tools(),"messages":[{"role":"user","content":"hi"}]})).unwrap(), &[]).unwrap();
        let canonical = serde_json::to_value(&request).unwrap();
        let key = session::key(&request);
        let hash = session::prefix_hash(&request.messages);
        for plan in [
            session::Plan::Fresh,
            session::Plan::Resume(session::Checkpoint {
                session_id: "s".into(),
                resume_uuid: "u".into(),
                pending_tool_ids: vec![],
                prefix_hash: [0; 32],
            }),
        ] {
            let mut config = spawn::Config::new("/empty");
            assert_eq!(
                spawn::configured_system_prompt(&request, &plan, &config),
                spawn::system_prompt(&request, &plan)
            );
            config.align_tool_names = true;
            let aligned = spawn::configured_system_prompt(&request, &plan, &config);
            assert!(aligned.contains("### `mcp__oc__read`"));
            assert!(aligned.contains("\"const\":\"read\""));
            assert_eq!(serde_json::to_value(&request).unwrap(), canonical);
            assert_eq!(session::key(&request), key);
            assert_eq!(session::prefix_hash(&request.messages), hash);
        }
    }
}
