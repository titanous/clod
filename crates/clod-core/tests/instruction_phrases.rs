use clod_core::name_alignment::{Registry, LIBRARY_INTRO};
use serde_json::json;
#[test]
fn only_validated_library_intro_callable_spans_change() {
    let registry = Registry::new(&[
        json!({"name":"job_block","description":"Wait."}),
        json!({"name":"job_status","description":"Status."}),
        json!({"name":"job_result","description":"Result."}),
    ]);
    let text = format!("## Tool Library{LIBRARY_INTRO}### `job_block`\n\nWait.");
    let expected = text
        .replace(
            "use job_block, job_status, or job_result",
            "use mcp__oc__job_block, mcp__oc__job_status, or mcp__oc__job_result",
        )
        .replace("### `job_block`", "### `mcp__oc__job_block`");
    assert_eq!(registry.rewrite_instructions(&text), expected);
    assert_eq!(registry.rewrite_instructions(&expected), expected);
    let drift = text.replace("one at a time", "sequentially");
    assert_eq!(registry.rewrite_instructions(&drift), drift);
    for fenced in [
        format!("````markdown\n```\n{text}\n````"),
        format!("~~~~markdown\n~~~\n{text}\n~~~~"),
    ] {
        assert_eq!(registry.rewrite_instructions(&fenced), fenced);
    }
}
