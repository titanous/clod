//! Offline input-boundary properties. No child processes or live services.
use clod_core::{
    exchange::Exchange,
    protocol, replay, request, security,
    session::{Plan, SessionMap},
    spawn::{self, Config},
    sse,
};
use proptest::prelude::*;
use serde_json::{json, Value};

fn json_value() -> BoxedStrategy<Value> {
    prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(|n| json!(n)),
        ".{0,80}".prop_map(Value::String)
    ]
    .prop_recursive(4, 64, 8, |inner| {
        prop_oneof![
            proptest::collection::vec(inner.clone(), 0..8).prop_map(Value::Array),
            proptest::collection::btree_map(".{0,20}", inner, 0..8)
                .prop_map(|m| Value::Object(m.into_iter().collect()))
        ]
    })
    .boxed()
}
fn base() -> Value {
    json!({"model":"m","messages":[{"role":"user","content":"hello"}],"max_tokens":1})
}
fn parsed(value: &Value) -> request::Request {
    request::parse(&serde_json::to_vec(value).unwrap(), &[]).unwrap()
}
fn arg<'a>(spec: &'a spawn::SpawnSpec, flag: &str) -> Option<&'a str> {
    spec.args
        .iter()
        .position(|s| s == flag)
        .and_then(|i| spec.args.get(i + 1))
        .map(String::as_str)
}

#[test]
fn host_grammar_exhaustive() {
    // Independent construction: enumerate the accepted language, not implementation splitting.
    let names = [
        ("localhost", true),
        ("LOCALHOST", true),
        ("LocalHost", true),
        ("127.0.0.1", true),
        ("[::1]", true),
        ("127.0.0.2", false),
        ("127.255.255.255", false),
        ("localhost.", false),
        ("localhost.evil.com", false),
        ("evillocalhost", false),
        ("127.0.0.1.nip.io", false),
        ("a@127.0.0.1", false),
        ("[::ffff:127.0.0.1]", false),
        ("2130706433", false),
        ("0177.0.0.1", false),
        ("0x7f000001", false),
        ("127.1", false),
        ("", false),
        (" localhost", false),
        ("localhost ", false),
        ("%6cocalhost", false),
        ("::1", false),
        ("[::1", false),
    ];
    let ports = [
        ("", true),
        (":0", true),
        (":1", true),
        (":65535", true),
        (":00080", true),
        (":", false),
        (":65536", false),
        (":-1", false),
        (":http", false),
        (":80 ", false),
        (":80:90", false),
    ];
    let mut count = 0;
    for (name, n_ok) in names {
        for (port, p_ok) in ports {
            let host = format!("{name}{port}");
            for origin in [false, true] {
                for override_host in [false, true] {
                    assert_eq!(
                        security::inbound_allowed(origin, Some(&host), override_host),
                        !origin && (override_host || n_ok && p_ok),
                        "{host:?}"
                    );
                    count += 1;
                }
            }
        }
    }
    assert_eq!(count, 1012);
    for origin in [false, true] {
        for override_host in [false, true] {
            assert_eq!(
                security::inbound_allowed(origin, None, override_host),
                !origin && override_host
            );
        }
    }
}

#[test]
fn explicit_tool_boundaries_and_permissive_names() {
    for name in [
        "x".repeat(54),
        "x".repeat(55),
        "x".repeat(56),
        "é".repeat(55),
        "".into(),
        "mcp__oc__lookup".into(),
    ] {
        let mut value = base();
        value["tools"] = json!([{"name":name},{"name":name}]);
        let result = request::parse(&serde_json::to_vec(&value).unwrap(), &[]);
        assert_eq!(result.is_ok(), name.chars().count() <= 55);
        if name.chars().count() > 55 {
            assert!(result.unwrap_err().to_string().contains(&name));
            // Constructor also protects callers who construct Request directly.
            let direct: request::Request = serde_json::from_value(value).unwrap();
            let (_, actions) = Exchange::new(direct, &SessionMap::new(4), Config::new("fixed"), 0);
            assert!(!actions
                .iter()
                .any(|a| matches!(a, clod_core::exchange::Action::Spawn(_))));
        }
    }
}

proptest! {
    #[test]
    fn host_unconstrained_never_accepts_other_names(host in ".{0,100}", origin in any::<bool>()) {
        if security::inbound_allowed(origin,Some(&host),false) {
            prop_assert!(!origin);
            let allowed = ["localhost","127.0.0.1","[::1]"].iter().any(|n| {
                if host.eq_ignore_ascii_case(n) { return true; }
                let prefix = format!("{n}:");
                host.get(..prefix.len()).is_some_and(|p| p.eq_ignore_ascii_case(&prefix)) &&
                    host[prefix.len()..].parse::<u16>().is_ok() && host[prefix.len()..].bytes().all(|c| c.is_ascii_digit())
            });
            prop_assert!(allowed);
        }
    }
    #[test]
    fn env_prefix_contract(suffix in "[A-Za-z0-9_]{0,60}") {
        for prefix in ["CLOD_","ANTHROPIC_"] { let name = format!("{prefix}{suffix}"); prop_assert!(security::scrub_child_env(&name)); }
        for name in ["HOME","HTTPS_PROXY","https_proxy","HTTP_PROXY","ALL_PROXY","NO_PROXY","anthropic_API_KEY","clod_API_KEY","CLAUDE_CODE_USE_BEDROCK_EXTRA","CLAUDE_CODE_USE_VERTEX_EXTRA"] {
            prop_assert!(!security::scrub_child_env(name));
        }
        for name in ["CLAUDE_CODE_USE_BEDROCK","CLAUDE_CODE_USE_VERTEX"] { prop_assert!(security::scrub_child_env(name)); }
    }
    #[test]
    fn request_spawn_invariants(model in ".{0,80}", system in ".{0,80}", max in any::<u64>(), root in any::<bool>(), length in 0usize..80, thinking in prop_oneof![Just(Value::Null), Just(json!({"type":"disabled"})), Just(json!({"type":"adaptive"})), any::<u64>().prop_map(|b| json!({"type":"enabled","budget_tokens":b}))]) {
        let name = "x".repeat(length);
        let mut value = base(); value["model"] = json!(model); value["system"] = json!(system); value["max_tokens"] = json!(max); value["tools"] = json!([{"name":name}]); value["thinking"] = thinking;
        let result = request::parse(&serde_json::to_vec(&value).unwrap(),&[]);
        if length > 55 { prop_assert!(result.is_err()); prop_assert!(result.unwrap_err().to_string().contains(&name)); }
        else {
            let req = result.unwrap(); let mut config = Config::new("/fixed/workdir"); config.is_root = root;
            let spec = spawn::spec(&req,&Plan::Fresh,&config);
            prop_assert_eq!(&spec.working_directory,"/fixed/workdir");
            prop_assert_eq!(arg(&spec,"--model"),Some(model.as_str()));
            prop_assert_eq!(arg(&spec,"--max-turns"),Some("1"));
            prop_assert_eq!(arg(&spec,"--tools"),Some("--setting-sources="));
            prop_assert!(spec.args.contains(&"--strict-mcp-config".into()));
            let expected_tools = format!("mcp__oc__{name}");
            prop_assert_eq!(arg(&spec,"--allowedTools"),Some(expected_tools.as_str()));
            for (key,val) in [("CLAUDE_CODE_DISABLE_ATTACHMENTS","1".to_string()),("ENABLE_CLAUDEAI_MCP_SERVERS","false".into()),("ENABLE_TOOL_SEARCH","false".into()),("CLAUDE_CODE_MAX_OUTPUT_TOKENS",max.to_string())] {
                prop_assert!(spec.env_set.contains(&(key.into(),val)));
            }
            prop_assert_eq!(spec.env_set.contains(&("IS_SANDBOX".into(),"1".into())),root);
            for key in ["CLAUDE_CONFIG_DIR","ANTHROPIC_API_KEY","ANTHROPIC_AUTH_TOKEN","CLAUDE_CODE_OAUTH_TOKEN","CLAUDECODE","ANTHROPIC_BASE_URL"] { prop_assert!(spec.env_remove.iter().any(|k| k == key)); }
            prop_assert_eq!(spawn::system_prompt(&req,&Plan::Fresh),format!("{system}{}",replay::PROVENANCE_NOTE));
        }
    }
    #[test]
    fn arbitrary_requests_and_stdout_do_not_panic(value in json_value(), raw in ".{0,300}") {
        let body = serde_json::to_vec(&value).unwrap();
        if let Ok(req) = request::parse(&body,&[]) { let _ = spawn::spec(&req,&Plan::Fresh,&Config::new("fixed")); }
        let (mut exchange,_) = Exchange::new(parsed(&base()),&SessionMap::new(4),Config::new("fixed"),0);
        let _ = exchange.on_stdout_line(&raw,1);
        let _ = exchange.on_stdout_line(&value.to_string(),2);
    }
    #[test]
    fn sse_and_control_roundtrip(value in json_value(), id in ".{0,100}", event in "[a-z_]{1,30}") {
        let encoded = sse::encode(&event,&value);
        let lines: Vec<_> = encoded.lines().collect();
        prop_assert_eq!(lines.len(),3);
        prop_assert_eq!(lines[0],format!("event: {event}"));
        prop_assert_eq!(serde_json::from_str::<Value>(lines[1].strip_prefix("data: ").unwrap()).unwrap(),value.clone());
        match protocol::parse(&protocol::control(&id,value.clone())).unwrap() {
            protocol::Record::ControlRequest(r) => { prop_assert_eq!(r.request_id,id); prop_assert_eq!(r.request,value); }
            _ => prop_assert!(false),
        }
    }
}
