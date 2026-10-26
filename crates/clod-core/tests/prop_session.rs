//! Contract models: PLAN Phase 2 session, updated by protocol.md's consecutive-user rule.
use clod_core::{
    request::{self, Content, Message, Request},
    session::{self, Checkpoint, Final, FreshReason, KeyMode, MapEffect, Plan, SessionMap},
};
use proptest::prelude::*;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};

static COVERAGE: [AtomicUsize; 12] = [const { AtomicUsize::new(0) }; 12];
fn count(i: usize) {
    COVERAGE[i].fetch_add(1, Ordering::Relaxed);
}
fn cp(v: u8) -> Checkpoint {
    Checkpoint {
        session_id: format!("s{v}"),
        resume_uuid: format!("u{v}"),
        pending_tool_ids: vec![],
        prefix_hash: [v; 32],
    }
}

#[derive(Clone, Debug)]
enum Op {
    Insert(u8, u8),
    Get(u8),
    Touch(u8),
    Remove(u8),
    Apply(Option<u8>, u8, u8),
    Size,
}
fn ops() -> impl Strategy<Value = Vec<Op>> {
    (0u8..3).prop_flat_map(|swarm| {
        let write = if swarm == 0 { 10 } else { 3 };
        let use_weight = if swarm == 1 { 10 } else { 3 };
        prop::collection::vec(prop_oneof![
            write => (0u8..5,any::<u8>()).prop_map(|(k,v)|Op::Insert(k,v)),
            use_weight => (0u8..5).prop_map(Op::Get),
            use_weight => (0u8..5).prop_map(Op::Touch),
            3 => (0u8..5).prop_map(Op::Remove),
            6 => (prop::option::of(0u8..5),0u8..3,any::<u8>()).prop_map(|(k,e,v)|Op::Apply(k,e,v)),
            1 => Just(Op::Size),
        ], 1..120)
    })
}
fn run_map(cap: usize, ops: &[Op]) {
    let mut actual = SessionMap::new(cap);
    let mut model: Vec<(u8, Checkpoint)> = vec![]; // oldest first; reads do not count as use.
    let mut removed = [false; 5];
    for op in ops {
        let mut insertion = None;
        match *op {
            Op::Insert(k, v) => {
                actual.insert([k; 32], cp(v));
                insertion = Some((k, v));
            }
            Op::Get(k) => {
                assert_eq!(
                    actual.get(&[k; 32]),
                    model.iter().find(|(id, _)| *id == k).map(|(_, c)| c)
                );
            }
            Op::Touch(k) => {
                actual.touch(&[k; 32]);
                if let Some(i) = model.iter().position(|(id, _)| *id == k) {
                    let item = model.remove(i);
                    model.push(item);
                }
            }
            Op::Remove(k) => {
                actual.remove(&[k; 32]);
                removed[k as usize] = true;
                model.retain(|(id, _)| *id != k);
            }
            Op::Apply(k, e, v) => {
                session::apply(
                    &mut actual,
                    Final {
                        key: k.map(|k| [k; 32]),
                        map_effect: match e {
                            0 => MapEffect::Commit(cp(v)),
                            1 => MapEffect::Keep,
                            _ => MapEffect::Discard,
                        },
                    },
                );
                if let Some(k) = k {
                    match e {
                        0 => insertion = Some((k, v)),
                        1 => (),
                        _ => {
                            removed[k as usize] = true;
                            model.retain(|(id, _)| *id != k);
                        }
                    }
                }
            }
            Op::Size => (),
        }
        if let Some((k, v)) = insertion {
            if cap > 0 {
                if model.iter().any(|(id, _)| *id == k) {
                    count(0);
                }
                if removed[k as usize] {
                    count(2);
                    removed[k as usize] = false;
                }
                model.retain(|(id, _)| *id != k);
                model.push((k, cp(v)));
                if model.len() > cap {
                    let (k, _) = model.remove(0);
                    removed[k as usize] = true;
                    count(1);
                }
            }
        }
        assert_eq!(actual.len(), model.len(), "{op:?}");
        assert_eq!(actual.is_empty(), model.is_empty());
        for k in 0..5 {
            assert_eq!(
                actual.get(&[k; 32]),
                model.iter().find(|(id, _)| *id == k).map(|(_, c)| c),
                "{op:?}, cap={cap}"
            );
        }
    }
}
fn message(role: &str, content: Content) -> Message {
    Message {
        role: role.into(),
        content,
    }
}
fn base() -> Request {
    request::parse(&serde_json::to_vec(&json!({"model":"m","system":"s","max_tokens":10,"tools":[{"name":"a","input_schema":{}},{"name":"b","input_schema":{}}],"messages":[{"role":"user","content":"hi"},{"role":"assistant","content":"answer"}]})).unwrap(),&[("X-Polytoken-Conversation","c")]).unwrap()
}
fn result(id: &str) -> Value {
    json!({"type":"tool_result","tool_use_id":id,"content":"ok"})
}

// Independent interpreter: find the assistant boundary by consuming user messages
// backwards, then validate the whole result multiset and its position before text.
// Hashing is deliberately shared; canonical-history properties belong to a separate harness.
fn interpret(
    r: &Request,
    saved_key: session::SessionKey,
    saved: Option<&Checkpoint>,
) -> Option<FreshReason> {
    let Some(k) = session::key(r) else {
        return Some(FreshReason::NoKey);
    };
    let Some(c) = saved.filter(|_| k == saved_key) else {
        return Some(FreshReason::NoCheckpoint);
    };
    if c.session_id.trim().is_empty() || c.resume_uuid.trim().is_empty() {
        return Some(FreshReason::InvalidCheckpoint);
    }
    let mut boundary = r.messages.len();
    while boundary > 0 && r.messages[boundary - 1].role == "user" {
        boundary -= 1;
    }
    if boundary == r.messages.len() {
        return Some(FreshReason::NotSingleUserTail);
    }
    let hash = session::prefix_hash(&r.messages[..boundary]);
    if hash != c.prefix_hash {
        return Some(FreshReason::PrefixMismatch {
            expected: c.prefix_hash,
            actual: hash,
        });
    }
    let tail = &r.messages[boundary..];
    if !c.pending_tool_ids.is_empty() && tail.iter().all(|m| matches!(m.content, Content::Text(_)))
    {
        return Some(FreshReason::TextWithPending);
    }
    let mut ids = vec![];
    let mut text_position = None;
    let mut bad = false;
    for block in tail
        .iter()
        .flat_map(|m| match &m.content {
            Content::Text(t) => vec![json!({"type":"text","text":t})],
            Content::Blocks(b) => b.clone(),
        })
        .enumerate()
    {
        let (position, b) = block;
        match b["type"].as_str() {
            Some("text") => {
                text_position.get_or_insert(position);
            }
            Some("tool_result") => {
                if text_position.is_some() {
                    bad = true;
                }
                if let Some(id) = b["tool_use_id"].as_str() {
                    ids.push(id.to_owned());
                } else {
                    bad = true;
                }
            }
            _ => {
                bad |= !c.pending_tool_ids.is_empty();
            }
        }
    }
    let mut pending = c.pending_tool_ids.clone();
    pending.sort();
    ids.sort();
    bad |= pending.windows(2).any(|w| w[0] == w[1])
        || ids.windows(2).any(|w| w[0] == w[1])
        || ids != pending;
    if bad {
        Some(FreshReason::PendingMismatch)
    } else {
        None
    }
}
fn planner_case(n: usize, variant: u8, split: usize, reverse: bool, text: String) {
    let mut r = base();
    let saved_key = session::key(&r).unwrap();
    let mut c = cp(0);
    c.prefix_hash = session::prefix_hash(&r.messages);
    c.pending_tool_ids = (0..n).map(|i| format!("id{i}")).collect();
    let mut blocks: Vec<_> = c.pending_tool_ids.iter().map(|id| result(id)).collect();
    if reverse {
        blocks.reverse();
    }
    blocks.push(json!({"type":"text","text":text}));
    match variant {
        1 if n > 0 => {
            blocks.remove(0);
        }
        2 if n > 0 => {
            blocks.insert(0, blocks[0].clone());
        }
        3 => {
            blocks.insert(0, result("unknown"));
        }
        4 => {
            blocks.insert(0, json!({"type":"text","text":"early"}));
        }
        5 => {
            r.messages[0].content = Content::Text("edited".into());
        }
        6 => r.model.push('x'),
        7 => r.system = Content::Text("different".into()),
        8 => r.tools[0]["description"] = json!("different"),
        9 => r.conversation_id = None,
        10 => c.resume_uuid = " \t".into(),
        11 => blocks.clear(),
        12 => {
            if n > 0 {
                c.pending_tool_ids.push(c.pending_tool_ids[0].clone());
            }
        }
        13 => blocks = vec![json!({"type":"tool_result"})],
        14 => blocks = vec![json!({"type":"image","source":{}})],
        _ => (),
    }
    if variant == 15 {
        r.messages.push(message("user", Content::Text(text)));
    } else {
        let cut = split.min(blocks.len());
        r.messages
            .push(message("user", Content::Blocks(blocks[..cut].to_vec())));
        r.messages
            .push(message("user", Content::Blocks(blocks[cut..].to_vec())));
    }
    if variant == 16 {
        r.messages.last_mut().unwrap().role = "assistant".into();
    }
    if variant == 17 {
        r.messages.insert(
            2,
            message("assistant", Content::Blocks(vec![result("id0")])),
        );
    }
    let mut map = SessionMap::new(2);
    if variant != 18 {
        map.insert(saved_key, c.clone());
    }
    let expected = interpret(&r, saved_key, if variant == 18 { None } else { Some(&c) });
    assert_eq!(
        session::fresh_reason(&r, &map, KeyMode::Strict),
        expected,
        "n={n}, variant={variant}, split={split}"
    );
    assert_eq!(
        session::plan(&r, &map),
        if expected.is_none() {
            Plan::Resume(c.clone())
        } else {
            Plan::Fresh
        }
    );
    let i = match expected {
        None => 3,
        Some(FreshReason::NoKey) => 4,
        Some(FreshReason::NoCheckpoint) => 5,
        Some(FreshReason::InvalidCheckpoint) => 6,
        Some(FreshReason::NotSingleUserTail) => 7,
        Some(FreshReason::PrefixMismatch { .. }) => 8,
        Some(FreshReason::PendingMismatch) => 9,
        Some(FreshReason::TextWithPending) => 10,
    };
    count(i);
    // Splitting the same flattened content must not affect resume eligibility.
    let (prefix, tail) = session::user_tail(&r.messages);
    if !tail.is_empty() {
        let flat = tail
            .iter()
            .flat_map(|m| match &m.content {
                Content::Text(t) => vec![json!({"type":"text","text":t})],
                Content::Blocks(b) => b.clone(),
            })
            .collect();
        let mut merged = r.clone();
        merged.messages = prefix.to_vec();
        merged.messages.push(message("user", Content::Blocks(flat)));
        assert_eq!(session::plan(&r, &map), session::plan(&merged, &map));
    }
}
proptest! {
    #![proptest_config(ProptestConfig { failure_persistence: Some(Box::new(proptest::test_runner::FileFailurePersistence::Direct("tests/proptest-regressions/prop_session.txt"))), ..ProptestConfig::default() })]
    #[test]
    fn map_model(cap in 0usize..8, operations in ops()) {run_map(cap,&operations);}
    #[test]
    fn planner_model(n in 0usize..5,variant in 0u8..19,split in 0usize..7,reverse in any::<bool>(),text in "[a-z]{0,8}") {planner_case(n,variant,split,reverse,text);}
    #[test]
    fn key_isolation(s in "[a-z]{1,8}", component in 0u8..5) {
        let a=base(); let mut b=a.clone();
        match component {0=>b.conversation_id=Some(format!("c{s}")),1=>b.model.push_str(&s),2=>b.system=Content::Text(format!("s{s}")),3=>b.tools[0]["description"]=json!(s),_=>b.tools[0]["input_schema"]=json!({"title":s})}
        prop_assert_ne!(session::key(&a),session::key(&b));
        b=a.clone(); b.tools.reverse(); prop_assert_eq!(session::key(&a),session::key(&b));
        b=a.clone(); b.conversation_id=Some(format!("c{s}")); b.model="m".into();
        let mut moved=a.clone(); moved.conversation_id=Some("c".into()); moved.model=format!("{s}m");
        prop_assert_ne!(session::key(&b),session::key(&moved));
    }
}
#[test]
fn calibration_reproductions() {
    // Eviction must retain the newly committed entry, not the old one.
    run_map(1, &[Op::Insert(0, 0), Op::Apply(Some(1), 0, 0)]);
    // Keep preserves contents and recency, observable through the next eviction.
    run_map(
        2,
        &[
            Op::Insert(0, 0),
            Op::Insert(1, 1),
            Op::Apply(Some(0), 1, 0),
            Op::Insert(2, 2),
        ],
    );
    // One pending call, duplicated result; and two valid results in reverse order.
    planner_case(1, 2, 0, false, String::new());
    planner_case(2, 0, 1, true, String::new());
    let a = base();
    let mut b = a.clone();
    b.model = "another-model".into();
    assert_ne!(session::key(&a), session::key(&b));
}
#[test]
fn bounded_map_sequences() {
    let alphabet = [
        Op::Insert(0, 1),
        Op::Insert(1, 2),
        Op::Insert(2, 3),
        Op::Get(0),
        Op::Touch(0),
        Op::Remove(1),
        Op::Apply(Some(0), 1, 0),
        Op::Apply(None, 0, 4),
    ];
    for cap in 0..=2 {
        for code in 0usize..8usize.pow(4) {
            let mut code = code;
            let mut ops = vec![];
            for _ in 0..4 {
                ops.push(alphabet[code % 8].clone());
                code /= 8;
            }
            run_map(cap, &ops);
        }
    }
}
#[test]
fn coverage_matrix() {
    for n in 0..5 {
        for variant in 0..19 {
            for split in 0..7 {
                planner_case(n, variant, split, true, String::new());
            }
        }
    }
    run_map(
        2,
        &[
            Op::Insert(0, 1),
            Op::Insert(1, 2),
            Op::Insert(0, 3),
            Op::Insert(2, 4),
            Op::Insert(1, 5),
            Op::Remove(0),
            Op::Insert(0, 6),
        ],
    );
    let totals: Vec<_> = COVERAGE
        .iter()
        .take(11)
        .map(|c| c.load(Ordering::Relaxed))
        .collect();
    eprintln!("coverage [overwrite, eviction, reinsert, resume, no_key, no_checkpoint, invalid_cp, no_tail, prefix, pending, text_pending]: {totals:?}");
    assert!(totals.iter().all(|n| *n > 0));
}
