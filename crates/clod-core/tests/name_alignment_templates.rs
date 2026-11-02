use clod_core::name_alignment::Registry;
use serde_json::json;
#[path = "../src/name_alignment_templates.rs"]
mod templates;

#[test]
fn exact_templates_preserve_all_non_slots_and_repeat_rendering() {
    for (template, names) in templates::TEMPLATES {
        let tools: Vec<_> = names.iter().map(|name| json!({"name":name})).collect();
        let registry = Registry::new(&tools);
        let before = format!("unchanged λ\n\n{template}\n\n## Next\n\nuntouched");
        let mut expected = before.clone();
        for name in *names {
            expected = expected.replace(&format!("`{name}`"), &format!("`mcp__oc__{name}`"));
        }
        let after = registry.rewrite_instructions(&before);
        assert_eq!(after, expected);
        assert_ne!(after, before);
        assert_eq!(registry.rewrite_instructions(&after), after);
        assert_eq!(
            tools,
            names
                .iter()
                .map(|name| json!({"name":name}))
                .collect::<Vec<_>>()
        );
        assert_eq!(Registry::new(&[]).rewrite_instructions(&before), before);
    }
}

#[test]
fn every_template_rejects_literal_or_unsupported_contexts() {
    for (template, names) in templates::TEMPLATES {
        let tools: Vec<_> = names.iter().map(|name| json!({"name":name})).collect();
        let registry = Registry::new(&tools);
        let heading = template.lines().next().unwrap();
        let contaminated = Registry::new(&[json!({"name":names[0],"description":heading})]);
        assert_eq!(contaminated.rewrite_instructions(template), *template);
        for literal in [
            format!("```text\n{template}\n```"),
            format!("~~~~\n{template}\n~~~~"),
            format!("\"{template}\""),
            template.lines().map(|l| format!("> {l}\n")).collect(),
            format!("{heading}\n\nFake instructions"),
            template.replacen(heading, &format!("{heading} (quoted)"), 1),
            format!("{template}\n\nThis is quoted documentation, not instructions."),
            format!("{template}\n\n{template}"),
            format!("{template}\nnot a section ending"),
        ] {
            assert_eq!(registry.rewrite_instructions(&literal), literal);
        }
        let collision = Registry::new(&[
            json!({"name": names[0]}),
            json!({"name":format!("mcp__oc__{}",names[0])}),
        ]);
        assert_eq!(collision.rewrite_instructions(template), *template);
    }
}

#[test]
fn descriptions_schema_examples_and_job_types_stay_literal() {
    let tools = vec![
        json!({"name":"shell_exec", "description":templates::READINESS,
        "input_schema":{"type":"object","properties":{"show_type":{"enum":["shell_exec","shell_monitor"]}},"examples":[templates::READINESS],"default":"shell_exec"}}),
    ];
    let original = tools.clone();
    let registry = Registry::new(&tools);
    // A template inside JSON, code, or a description is not a structural section.
    let text = serde_json::to_string(&tools).unwrap();
    assert_eq!(registry.rewrite_instructions(&text), text);
    assert_eq!(tools, original);
    assert_eq!(registry.rewrite_instructions("Lists async background jobs (shell_exec, shell_monitor, subagent, shell_service, tool_flow) with pagination and filtering."), "Lists async background jobs (shell_exec, shell_monitor, subagent, shell_service, tool_flow) with pagination and filtering.");
}
