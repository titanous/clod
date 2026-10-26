use clod_core::{
    exchange::{Action, Exchange},
    request,
    session::{self, MapEffect, Plan, SessionMap},
    spawn::Config,
};
#[test]
fn captured_empty_delta_and_declared_native_tool_commit() {
    for (fixture, commits, reason) in [
        (include_str!("fixtures/live/empty-input.ndjson"), true, None),
        (include_str!("fixtures/live/bare-lookup.ndjson"), true, None),
        (
            include_str!("fixtures/live/native-search.ndjson"),
            true,
            None,
        ),
    ] {
        let request=request::parse(br#"{"model":"m","max_tokens":64,"stream":true,"messages":[{"role":"user","content":"test"}],"tools":[{"name":"tool_search","input_schema":{"type":"object"}},{"name":"lookup","input_schema":{"type":"object"}}]}"#,&[("x-polytoken-conversation","c")]).unwrap();
        let (mut e, _) = Exchange::new(
            request.clone(),
            &SessionMap::default(),
            Config::new("/empty"),
            0,
        );
        let mut actions = Vec::new();
        for (i, line) in fixture.lines().enumerate() {
            actions.extend(e.on_stdout_line(line, i as u64 + 1));
        }
        actions.extend(e.on_stdout_eof(100));
        actions.extend(e.on_stderr_eof(101));
        actions.extend(e.on_exit(0, 102));
        let effect = actions
            .iter()
            .find_map(|a| {
                if let Action::Done(f) = a {
                    Some(&f.map_effect)
                } else {
                    None
                }
            })
            .unwrap();
        assert_eq!(
            matches!(effect, MapEffect::Commit(_)),
            commits,
            "{effect:?}"
        );
        assert_eq!(e.checkpoint_reason(), reason);
        if commits {
            let mut blocks = Vec::new();
            for action in &actions {
                if let Action::Emit(event) = action {
                    if event.event == "content_block_start" {
                        blocks.push(event.data["content_block"].clone());
                    } else if event.event == "content_block_delta" {
                        let delta = &event.data["delta"];
                        if delta["type"] == "text_delta" {
                            let block = blocks.last_mut().unwrap();
                            let text = block["text"].as_str().unwrap_or("").to_owned()
                                + delta["text"].as_str().unwrap_or("");
                            block["text"] = serde_json::json!(text);
                        }
                        if delta["type"] == "thinking_delta" {
                            let block = blocks.last_mut().unwrap();
                            let text = block["thinking"].as_str().unwrap_or("").to_owned()
                                + delta["thinking"].as_str().unwrap_or("");
                            block["thinking"] = serde_json::json!(text);
                        }
                    }
                }
            }
            let fragment: String = actions
                .iter()
                .filter_map(|a| match a {
                    Action::Emit(event) if event.data["delta"]["type"] == "input_json_delta" => {
                        event.data["delta"]["partial_json"].as_str()
                    }
                    _ => None,
                })
                .collect();
            if !fragment.is_empty() {
                blocks.iter_mut().find(|b| b["type"] == "tool_use").unwrap()["input"] =
                    serde_json::from_str(&fragment).unwrap();
            }
            let tool = blocks.iter().find(|b| b["type"] == "tool_use").unwrap();
            if fixture.contains("tool_search") {
                assert_eq!(tool["name"], "tool_search");
                assert_eq!(
                    tool["input"],
                    serde_json::json!({"query":"read files","max_results":"3"})
                );
            } else if fixture.contains("\"name\":\"lookup\"") {
                assert_eq!(tool["name"], "lookup");
                assert_eq!(tool["input"], serde_json::json!({"key":"alpha"}));
                assert_eq!(tool["id"], "toolu_01Qva23KkpYqaUuuEhBRHNgd");
            } else {
                assert_eq!(tool["input"], serde_json::json!({}));
            }
            let id = tool["id"].clone();
            let mut echoed = request;
            echoed.messages.push(request::Message {
                role: "assistant".into(),
                content: request::Content::Blocks(blocks),
            });
            echoed.messages.push(request::Message {
                role: "user".into(),
                content: request::Content::Blocks(vec![
                    serde_json::json!({"type":"tool_result","tool_use_id":id,"content":"ok"}),
                ]),
            });
            let final_state = actions
                .iter()
                .find_map(|a| {
                    if let Action::Done(f) = a {
                        Some(f.clone())
                    } else {
                        None
                    }
                })
                .unwrap();
            if let MapEffect::Commit(cp) = &final_state.map_effect {
                assert_eq!(cp.pending_tool_ids, vec![id.as_str().unwrap().to_owned()]);
                if fixture.contains("tool_search") {
                    assert_eq!(cp.resume_uuid, "c91e8d3e-8691-403c-8d40-32d0faa31efe");
                } else if fixture.contains("\"name\":\"lookup\"") {
                    assert_eq!(cp.resume_uuid, "e14cdb3d-9e7d-4691-a75d-de11233106ac");
                }
                assert_eq!(
                    cp.prefix_hash,
                    session::prefix_hash(&echoed.messages[..echoed.messages.len() - 1]),
                    "committed input must equal the echoed empty object"
                );
            }
            let mut map = SessionMap::default();
            session::apply(&mut map, final_state);
            assert!(matches!(session::plan(&echoed, &map), Plan::Resume(_)));
        }
    }
}
