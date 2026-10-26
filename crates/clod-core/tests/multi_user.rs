use clod_core::{
    exchange::{Action, Exchange},
    request::{self, Content, Message},
    session::{self, Checkpoint, Plan, SessionMap},
    spawn::Config,
};
use serde_json::json;

#[test]
fn recorded_image64_second_request_resumes() {
    let bodies: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("fixtures/live/image64-requests.json")).unwrap();
    let parse = |v: &serde_json::Value| {
        request::parse(
            &serde_json::to_vec(v).unwrap(),
            &[("x-polytoken-conversation", "image64")],
        )
        .unwrap()
    };
    let first = parse(&bodies[0]);
    let second = parse(&bodies[1]);
    let (prefix, tail) = session::user_tail(&second.messages);
    assert_eq!(tail.len(), 2);
    assert_eq!(&prefix[..first.messages.len()], first.messages.as_slice());
    assert_eq!(prefix.last().unwrap().role, "assistant");
    let mut map = SessionMap::default();
    map.insert(
        session::key(&first).unwrap(),
        Checkpoint {
            session_id: "captured-session".into(),
            resume_uuid: "captured-assistant".into(),
            pending_tool_ids: vec![],
            prefix_hash: session::prefix_hash(prefix),
        },
    );
    assert!(matches!(session::plan(&second, &map), Plan::Resume(_)));
}

#[test]
fn consecutive_users_merge_and_validate_results() {
    let base = request::parse(br#"{"model":"m","max_tokens":64,"messages":[{"role":"user","content":"start"},{"role":"assistant","content":"answer"}]}"#, &[("x-polytoken-conversation","multi")]).unwrap();
    let mut map = SessionMap::default();
    let mut cp = Checkpoint {
        session_id: "s".into(),
        resume_uuid: "u".into(),
        pending_tool_ids: vec![],
        prefix_hash: session::prefix_hash(&base.messages),
    };
    map.insert(session::key(&base).unwrap(), cp.clone());
    let mut image = base.clone();
    image.messages.push(Message { role:"user".into(), content:Content::Blocks(vec![json!({"type":"image","source":{"type":"base64","media_type":"image/png","data":"test"}})]) });
    image.messages.push(Message {
        role: "user".into(),
        content: Content::Text("describe".into()),
    });
    assert!(matches!(session::plan(&image, &map), Plan::Resume(_)));
    let (_, actions) = Exchange::new(image.clone(), &map, Config::new("/empty"), 0);
    let user = actions
        .iter()
        .filter_map(|a| match a {
            Action::WriteStdin(s) => serde_json::from_str::<serde_json::Value>(s).ok(),
            _ => None,
        })
        .find(|v| v["type"] == "user")
        .unwrap();
    assert_eq!(user["message"]["content"][0]["type"], "image");
    assert_eq!(user["message"]["content"][1]["text"], "describe");
    image.messages.insert(
        image.messages.len() - 1,
        Message {
            role: "assistant".into(),
            content: Content::Text("interposed".into()),
        },
    );
    assert_eq!(session::plan(&image, &map), Plan::Fresh);
    cp.pending_tool_ids = vec!["a".into(), "b".into()];
    map.insert(session::key(&base).unwrap(), cp);
    let mut results = base;
    for id in ["a", "b"] {
        results.messages.push(Message {
            role: "user".into(),
            content: Content::Blocks(vec![
                json!({"type":"tool_result","tool_use_id":id,"content":"ok"}),
            ]),
        });
    }
    assert!(matches!(session::plan(&results, &map), Plan::Resume(_)));
    for id in ["a", "unknown"] {
        let mut bad = results.clone();
        bad.messages.last_mut().unwrap().content = Content::Blocks(vec![
            json!({"type":"tool_result","tool_use_id":id,"content":"ok"}),
        ]);
        assert_eq!(session::plan(&bad, &map), Plan::Fresh);
    }
    let mut no_pending = map.clone();
    let mut cp = no_pending
        .get(&session::key(&results).unwrap())
        .unwrap()
        .clone();
    cp.pending_tool_ids.clear();
    no_pending.insert(session::key(&results).unwrap(), cp);
    assert_eq!(session::plan(&results, &no_pending), Plan::Fresh);
}
