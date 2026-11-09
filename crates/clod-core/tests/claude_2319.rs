use clod_core::protocol;
use serde_json::Value;

// Sanitized live captures; opaque signatures and private initialization values are redacted.
const CAPTURE: &str = include_str!("fixtures/live/claude-2.1.319-phase0.ndjson");

#[test]
fn refreshed_cli_protocol_shapes_parse() {
    let rows: Vec<Value> = CAPTURE
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    for row in rows.iter().filter(|row| row["dir"] == "out") {
        protocol::parse(&row["line"].to_string()).unwrap();
    }
    for scenario in ["enabled", "adaptive", "signed-resume"] {
        let selected: Vec<_> = rows
            .iter()
            .filter(|row| row["scenario"] == scenario)
            .collect();
        assert!(selected
            .iter()
            .any(|row| row["dir"] == "in" && row["line"]["request"]["subtype"] == "initialize"));
        assert!(selected.iter().any(|row| row["dir"] == "out"
            && row["line"]["response"]["response"]["claude_code_version"] == "2.1.319"
            && row["line"]["response"]["response"]["hooks_applied"] == true));
        let methods: Vec<_> = selected
            .iter()
            .filter_map(|row| row["line"]["request"]["message"]["method"].as_str())
            .collect();
        assert_eq!(
            methods,
            ["initialize", "notifications/initialized", "tools/list"]
        );
        assert!(selected.iter().any(|row| {
            row["line"]["message"]["content"]
                .as_array()
                .is_some_and(|blocks| {
                    blocks.iter().any(|block| {
                        block["type"] == "thinking"
                            && !block["thinking"].as_str().unwrap().is_empty()
                            && block["signature"].as_str().unwrap().contains("REDACTED")
                    })
                })
        }));
    }
    assert!(rows
        .iter()
        .any(|row| row["scenario"] == "adaptive"
            && row["line"]["request"]["subtype"] == "hook_callback"));
    assert!(rows.iter().any(|row| row["scenario"] == "adaptive"
        && row["line"]["response"]["response"]["hookSpecificOutput"]["permissionDecision"]
            == "deny"));
    for scenario in ["missing-session", "missing-uuid"] {
        let terminal = rows
            .iter()
            .find(|row| row["scenario"] == scenario && row["line"]["type"] == "result")
            .unwrap();
        assert_eq!(terminal["line"]["subtype"], "error_during_execution");
        assert_eq!(terminal["line"]["is_error"], true);
        assert_eq!(terminal["line"]["num_turns"], 0);
    }
}

#[test]
fn refreshed_capture_provenance_and_redactions_remain_explicit() {
    let report: Value =
        serde_json::from_str(include_str!("fixtures/live/claude-2.1.319-validation.json")).unwrap();
    assert_eq!(report["phase_forwarded_attempts"], 3);
    assert_eq!(report["aggregate_forwarded_attempts"], 15);
    assert!(!CAPTURE.contains("/Users/"));
    assert!(!CAPTURE.contains("/private/var/"));
    assert!(!CAPTURE.contains("sk-ant-"));
    assert!(CAPTURE.contains("<INITIALIZATION_VALUE_REDACTED>"));
    for scenario in report["scenarios"].as_array().unwrap().iter().take(3) {
        assert_eq!(scenario["cli_client_signed_blocks_equal"], true);
        assert_eq!(scenario["prior_signed_blocks_upstream_equal"], true);
    }
    assert_eq!(
        report["scenarios"][1]["thinking_signature_deltas_equal"],
        43
    );
}
