//! Real diagnostic capture, not synthetic stdout. See fixtures/live/README.md.
use clod_core::{
    exchange::{Action, Exchange},
    request,
    session::{self, MapEffect, Plan, SessionMap},
    spawn::Config,
};
use serde_json::{json, Value};

#[test]
fn captured_live_tool_turn_commits_and_echo_resumes() {
    let bodies: Vec<Value> =
        serde_json::from_str(include_str!("fixtures/live/requests.json")).unwrap();
    let parse = |body: &Value| {
        request::parse(
            &serde_json::to_vec(body).unwrap(),
            &[("x-polytoken-conversation", "captured-conversation")],
        )
        .unwrap()
    };
    let first = parse(&bodies[0]);
    let next = parse(&bodies[1]);
    assert_eq!(session::key(&first), session::key(&next));
    let mut map = SessionMap::default();
    assert_eq!(
        session::fresh_reason(&first, &map, session::KeyMode::Strict),
        Some(session::FreshReason::NoCheckpoint)
    );
    let mut keyless = first.clone();
    keyless.conversation_id = None;
    assert_eq!(
        session::fresh_reason(&keyless, &map, session::KeyMode::Strict),
        Some(session::FreshReason::NoKey)
    );
    let (mut exchange, _) = Exchange::new(first, &map, Config::new("/empty"), 0);
    let mut actions = Vec::new();
    for (i, line) in include_str!("fixtures/live/tool-stdout.ndjson")
        .lines()
        .enumerate()
    {
        actions.extend(exchange.on_stdout_line(line, i as u64 + 1));
    }
    actions.extend(exchange.on_stdout_eof(100));
    actions.extend(exchange.on_stderr_eof(101));
    actions.extend(exchange.on_exit(0, 102));
    let final_state = actions
        .into_iter()
        .find_map(|a| {
            if let Action::Done(f) = a {
                Some(f)
            } else {
                None
            }
        })
        .unwrap();
    assert!(
        matches!(final_state.map_effect, MapEffect::Commit(_)),
        "{final_state:?}, {:?}",
        exchange.checkpoint_reason()
    );
    assert_eq!(exchange.checkpoint_reason(), None);
    session::apply(&mut map, final_state);
    assert_eq!(
        session::fresh_reason(&next, &map, session::KeyMode::Strict),
        None
    );
    assert!(matches!(session::plan(&next, &map), Plan::Resume(_)));
}

#[test]
fn caller_normalization_is_content_only() {
    let a = json!([{"role":"assistant","content":[{"type":"tool_use","id":"t","name":"read","input":{"caller":"kept"},"caller":{"type":"direct"}}]}]);
    let mut b = a.clone();
    b[0]["content"][0].as_object_mut().unwrap().remove("caller");
    let history = |v: &Value| {
        session::canonical_history(
            &serde_json::from_value::<Vec<request::Message>>(v.clone()).unwrap(),
        )
    };
    assert_eq!(history(&a), history(&b));
    b[0]["content"][0]["input"]["caller"] = json!("different");
    assert_ne!(history(&a), history(&b));
}
