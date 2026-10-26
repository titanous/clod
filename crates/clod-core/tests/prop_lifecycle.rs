//! Offline lifecycle discovery. Run with PROPTEST_CASES=20000 cargo test -p clod-core --test prop_lifecycle.
use clod_core::{
    exchange::{Action, Exchange},
    request,
    session::{self, Checkpoint, MapEffect, SessionMap},
    spawn::Config,
};
use proptest::prelude::*;
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};

mod lifecycle_round2;

#[derive(Clone, Debug)]
enum Op {
    Out(u8),
    Err(bool),
    StdoutEof,
    StderrEof,
    Exit,
    Tick(u64),
    Gone,
    SpawnFailed,
    StdinFailed,
}
fn operation() -> impl Strategy<Value = Op> {
    prop_oneof![8 => (0u8..22).prop_map(Op::Out), 2 => any::<bool>().prop_map(Op::Err),
        1 => Just(Op::StdoutEof), 1 => Just(Op::StderrEof), 1 => Just(Op::Exit),
        5 => prop::sample::select(vec![0,1,1999,2000,2001,4999,5000,5001,14999,15000,15001,59999,60000,60001,899999,900000,900001]).prop_map(Op::Tick),
        1 => Just(Op::Gone), 1 => Just(Op::SpawnFailed), 2 => Just(Op::StdinFailed)]
}
fn history() -> impl Strategy<Value = (bool, bool, Vec<Op>)> {
    (
        any::<bool>(),
        any::<bool>(),
        0u8..8,
        prop::collection::vec(operation(), 0..70),
        0u8..8,
    )
        .prop_map(|(stream, resume, swarm, mut ops, pattern)| {
            ops.retain(|o| match swarm {
                0 => !matches!(o, Op::Gone | Op::StdinFailed),
                1 => !matches!(o, Op::Out(8..=14)),
                2 => !matches!(o, Op::Tick(_)),
                _ => true,
            });
            let mut prefix = match pattern {
                0 => vec![Op::Out(6), Op::Out(8), Op::Gone],
                1 => vec![Op::StdoutEof, Op::Err(true), Op::StderrEof, Op::Exit],
                2 => vec![
                    Op::Out(0),
                    Op::Out(15),
                    Op::Tick(60000),
                    Op::Out(2),
                    Op::Out(15),
                ],
                3 => vec![Op::Gone, Op::StdinFailed, Op::Tick(2000)],
                4 => vec![Op::Out(6), Op::Out(8), Op::Tick(4999), Op::Tick(1)],
                5 => vec![Op::Err(true), Op::StdoutEof, Op::StderrEof, Op::Exit],
                6 => vec![Op::Tick(900000), Op::Tick(1999), Op::Tick(1)],
                _ => vec![],
            };
            prefix.append(&mut ops);
            (stream, resume, prefix)
        })
}
fn line(n: u8) -> String {
    let event = match n {
        0 => Some(
            json!({"type":"message_start","message":{"id":"m","type":"message","role":"assistant","content":[]}}),
        ),
        1 => Some(
            json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
        ),
        2 => Some(
            json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":1}}),
        ),
        3 => Some(json!({"type":"message_stop"})),
        4 => Some(
            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"OK"}}),
        ),
        5 => Some(
            json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"t","name":"mcp__oc__lookup","input":{}}}),
        ),
        _ => None,
    };
    if let Some(event) = event {
        return json!({"type":"stream_event","session_id":"fork","event":event}).to_string();
    }
    match n {
        6|7=>json!({"type":"assistant","uuid":if n==6 {json!("new")}else{json!(null)},"session_id":"fork","message":{"content":[{"type":"text","text":"OK"}]}}).to_string(),
        8..=14=> {
            let (sub,text)=match n {8=>("success",""),9=>("error_max_turns","Reached maximum number of turns (1)"),10=>("error_during_execution","rate limit exceeded"),11=>("error_during_execution","API Error: 401 authentication_error"),12=>("error_during_execution","Prompt is too long"),13=>("error_during_execution","No conversation found with session ID: old"),_=>("error_during_execution","unknown failure")};
            json!({"type":"result","subtype":sub,"session_id":"fork","errors":[text],"usage":{}}).to_string()
        },
        15=>json!({"type":"control_request","request_id":"hook","request":{"subtype":"hook_callback","callback_id":"hook_0","input":{"tool_name":"mcp__oc__lookup"},"tool_use_id":"t"}}).to_string(),
        16..=19=>json!({"type":"control_request","request_id":"mcp","request":{"subtype":"mcp_message","server_name":"oc","message":{"jsonrpc":"2.0","id":1,"method":(["initialize","notifications/initialized","tools/list","tools/call"][(n-16)as usize])}}}).to_string(),
        20=>"{bad json".into(), _=>json!({"type":"future_record","payload":true}).to_string()
    }
}
fn setup(stream: bool, resume: bool) -> (Exchange, Vec<Action>) {
    let r=request::parse(&serde_json::to_vec(&json!({"model":"haiku","system":"test","messages":[{"role":"user","content":"hello"}],"tools":[{"name":"lookup","description":"test","input_schema":{"type":"object"}}],"max_tokens":2048,"stream":stream})).unwrap(),&[("x-polytoken-conversation","property")]).unwrap();
    let mut map = SessionMap::default();
    if resume {
        map.insert(
            session::key(&r).unwrap(),
            Checkpoint {
                session_id: "source".into(),
                resume_uuid: "old".into(),
                prefix_hash: session::prefix_hash(&[]),
                pending_tool_ids: vec![],
            },
        );
    }
    Exchange::new(r, &map, Config::new("/empty"), 0)
}
#[derive(Debug, Clone, Copy, PartialEq)]
enum Effect {
    Keep,
    Discard,
    Commit,
}
fn effect(e: &MapEffect) -> Effect {
    match e {
        MapEffect::Keep => Effect::Keep,
        MapEffect::Discard => Effect::Discard,
        MapEffect::Commit(_) => Effect::Commit,
    }
}
#[derive(Debug)]
struct Model {
    stream: bool,
    resume: bool,
    spawns: usize,
    done: bool,
    out_eof: bool,
    err_eof: bool,
    exited: bool,
    spawn_failed: bool,
    committed: bool,
    sse: bool,
    error: bool,
    gone: bool,
    events: bool,
    stale: bool,
    result: bool,
    selected: Option<Effect>,
    close: Option<(u64, u64)>,
    killed: bool,
    caused: bool,
    last_emit: u64,
    assistant: Option<bool>,
    generating: bool,
    hooks: bool,
    last_stream: u64,
    invalid: bool,
    starts: usize,
    tools: bool,
    boundary: bool,
}
impl Model {
    fn new(stream: bool, resume: bool) -> Self {
        Self {
            stream,
            resume,
            spawns: 0,
            done: false,
            out_eof: false,
            err_eof: false,
            exited: false,
            spawn_failed: false,
            committed: false,
            sse: false,
            error: false,
            gone: false,
            events: false,
            stale: false,
            result: false,
            selected: None,
            close: None,
            killed: false,
            caused: false,
            last_emit: 0,
            assistant: None,
            generating: false,
            hooks: false,
            last_stream: 0,
            invalid: false,
            starts: 0,
            tools: false,
            boundary: false,
        }
    }
    fn observe(&mut self, o: &Op, now: u64) {
        if self.done {
            return;
        }
        match o {
            Op::StdoutEof => self.out_eof = true,
            Op::StderrEof => self.err_eof = true,
            Op::Exit => self.exited = true,
            Op::SpawnFailed => self.spawn_failed = true,
            Op::Out(n) if self.close.is_none() => match n {
                0..=5 => {
                    self.events = true;
                    self.last_stream = now;
                    if *n == 1 || *n == 5 {
                        self.generating = true;
                    }
                    if *n == 0 {
                        self.starts += 1;
                        self.invalid |= self.starts > 1;
                        self.generating = true;
                    }
                    if *n == 2 || *n == 3 {
                        self.generating = false;
                        self.hooks = false;
                        self.boundary = true;
                    }
                    if *n == 5 {
                        self.invalid = true;
                        self.tools = true;
                    }
                }
                6 | 7 => {
                    self.assistant = Some(*n == 6);
                    if !self.events {
                        self.generating = false;
                        self.hooks = false;
                        self.boundary = true;
                    }
                }
                15 => {
                    if self.generating {
                        self.hooks = true;
                    }
                }
                8 => {
                    self.result = true;
                    self.selected = Some(
                        if self.assistant == Some(true) && !self.invalid && !self.hooks {
                            Effect::Commit
                        } else {
                            Effect::Discard
                        },
                    );
                }
                9..=14 => {
                    self.result = true;
                    if *n == 13 || (*n == 9 && self.tools && self.boundary) {
                        self.stale |= *n == 13;
                        self.selected = Some(Effect::Discard);
                    }
                }
                _ => {}
            },
            // Reviewed policy: stale evidence overrides cancellation Keep.
            // Successful result classification remains fixed during cleanup.
            Op::Out(13) | Op::Err(true) if !self.result => {
                self.stale = true;
                self.selected = Some(Effect::Discard);
            }
            Op::Gone => {
                self.gone = true;
                if self.close.is_none() {
                    self.selected.get_or_insert(if self.stale {
                        Effect::Discard
                    } else {
                        Effect::Keep
                    });
                }
            }
            Op::Tick(_) if self.close.is_none() && now >= 900000 => {
                self.selected.get_or_insert(if self.stale {
                    Effect::Discard
                } else {
                    Effect::Keep
                });
            }
            Op::Tick(_)
                if self.close.is_none() && self.hooks && now - self.last_stream >= 60000 =>
            {
                self.invalid = true;
                self.generating = false;
                self.hooks = false;
            }
            _ => {}
        }
    }
    fn check(&mut self, actions: &[Action], now: u64) {
        if self.done {
            assert!(actions.is_empty(), "actions after Done: {actions:?}");
            return;
        }
        for a in actions {
            assert!(!self.done, "action after Done in same batch");
            match a {
                Action::Spawn(spec) => {
                    self.spawns += 1;
                    assert!(self.spawns <= 2);
                    if self.spawns == 1 {
                        assert_eq!(spec.args.iter().any(|x| x == "--resume"), self.resume);
                    } else {
                        assert!(
                            self.resume
                                && self.stale
                                && !self.committed
                                && self.out_eof
                                && self.err_eof
                                && self.exited,
                            "illegal retry: {self:?}"
                        );
                        assert!(!spec.args.iter().any(|x| x == "--resume"));
                        self.out_eof = false;
                        self.err_eof = false;
                        self.exited = false;
                        self.result = false;
                        self.events = false;
                        self.close = None;
                        self.killed = false;
                        self.caused = false;
                        self.assistant = None;
                        self.selected = None;
                        self.generating = false;
                        self.hooks = false;
                        self.last_stream = now;
                        self.invalid = false;
                        self.starts = 0;
                        self.tools = false;
                        self.boundary = false;
                    }
                }
                Action::StartSse => {
                    assert!(self.stream && !self.committed);
                    self.committed = true;
                    self.sse = true;
                }
                Action::RespondJson(status, _) => {
                    assert!(!self.committed && !self.gone);
                    self.committed = true;
                    self.error = *status != 200;
                }
                Action::Emit(e) => {
                    assert!(self.stream && self.sse && !self.gone);
                    if e.event == "error" {
                        self.error = true;
                    }
                    if e.event == "message_stop" {
                        assert!(!self.error);
                        assert!(self.result);
                    }
                    if e.event == "ping" {
                        assert!(now - self.last_emit >= 15000);
                        assert!(self.close.is_none());
                    }
                    self.last_emit = now;
                }
                Action::WriteStdin(s) => {
                    if s.contains("\"subtype\":\"interrupt\"") {
                        self.caused = true;
                    }
                }
                Action::CloseStdin => {
                    assert!(self.close.is_none());
                    let delay = if self.gone || now >= 900000 {
                        2000
                    } else {
                        5000
                    };
                    self.close = Some((now, delay));
                    if self.result || self.stale {
                        self.caused = true;
                    }
                }
                Action::Kill => {
                    assert!(!self.killed);
                    self.killed = true;
                    self.caused = true;
                }
                Action::Done(f) => {
                    assert!(self.spawn_failed || (self.out_eof && self.err_eof && self.exited));
                    let expected = self.selected.unwrap_or(
                        if self.stale
                            || (self.resume
                                && self.spawns == 1
                                && !self.events
                                && !self.result
                                && !self.caused
                                && !self.spawn_failed)
                        {
                            Effect::Discard
                        } else {
                            Effect::Keep
                        },
                    );
                    assert_eq!(effect(&f.map_effect), expected, "map effect: {self:?}");
                    self.done = true;
                    record(expected, self.spawns, self.killed);
                }
                _ => {}
            }
        }
    }
}
static COUNTS: [AtomicUsize; 5] = [const { AtomicUsize::new(0) }; 5];
fn record(e: Effect, spawns: usize, kill: bool) {
    COUNTS[e as usize].fetch_add(1, Ordering::Relaxed);
    if spawns == 2 {
        COUNTS[3].fetch_add(1, Ordering::Relaxed);
    }
    if kill {
        COUNTS[4].fetch_add(1, Ordering::Relaxed);
    }
}
fn apply(e: &mut Exchange, o: &Op, now: u64) -> Vec<Action> {
    match o {
        Op::Out(n) => e.on_stdout_line(&line(*n), now),
        Op::Err(stale) => e.on_stderr_line(
            if *stale {
                "No conversation found with session ID: old"
            } else {
                "diagnostic"
            },
            now,
        ),
        Op::StdoutEof => e.on_stdout_eof(now),
        Op::StderrEof => e.on_stderr_eof(now),
        Op::Exit => e.on_exit(1, now),
        Op::Tick(_) => e.on_tick(now),
        Op::Gone => e.on_client_gone(now),
        Op::SpawnFailed => e.on_spawn_failed("synthetic spawn failure", now),
        Op::StdinFailed => e.on_stdin_failed("synthetic write failure", now),
    }
}
fn replay(stream: bool, resume: bool, ops: &[Op], faithful: bool) {
    let (mut e, a) = setup(stream, resume);
    let mut m = Model::new(stream, resume);
    m.check(&a, 0);
    let mut now = 0;
    for o in ops.iter().cloned().chain([
        Op::StdoutEof,
        Op::StderrEof,
        Op::Exit,
        Op::StdoutEof,
        Op::StderrEof,
        Op::Exit,
        Op::Tick(900000),
        Op::Gone,
    ]) {
        if let Op::Tick(delta) = o {
            now += delta;
        }
        if faithful {
            if matches!(o, Op::Out(_)) && m.out_eof
                || matches!(o, Op::Err(_)) && m.err_eof
                || matches!(o, Op::Exit) && !(m.out_eof && m.err_eof)
                || matches!(o, Op::SpawnFailed)
                    && !(m.spawns == 1 && now == 0 && !m.events && !m.result && m.close.is_none())
            {
                continue;
            }
            let deadline = m.close.filter(|_| !m.killed && !m.done).map(|(t, d)| t + d);
            let force = matches!(o, Op::Tick(_)) && deadline.is_some_and(|d| now >= d);
            m.observe(&o, now);
            let a = apply(&mut e, &o, now);
            if force {
                assert!(
                    a.iter().any(|a| matches!(a, Action::Kill)),
                    "missed kill deadline: {o:?} {m:?}"
                );
            }
            m.check(&a, now);
        } else {
            let _ = apply(&mut e, &o, now);
        }
    }
    if faithful {
        assert!(m.done, "history failed to terminate: {m:?}");
    }
}
proptest! {
    #![proptest_config(ProptestConfig { failure_persistence: Some(Box::new(proptest::test_runner::FileFailurePersistence::Direct("tests/prop_lifecycle.proptest-regressions"))), ..ProptestConfig::default() })]
    #[test]
    fn shell_histories((stream,resume,ops) in history()){replay(stream,resume,&ops,true);}
    #[test]
    fn arbitrary_orders_do_not_panic(stream in any::<bool>(),resume in any::<bool>(),ops in prop::collection::vec(operation(),0..100)){replay(stream,resume,&ops,false);}
}
#[test]
fn explicit_discovery_histories() {
    // Existing stale-precedence policy; PLAN conflict documented in report.
    replay(false, true, &[Op::Gone, Op::Out(13)], true);
    // Backstop invalidates an otherwise complete text checkpoint.
    replay(
        false,
        false,
        &[
            Op::Out(0),
            Op::Out(15),
            Op::Tick(60000),
            Op::Out(2),
            Op::Out(15),
            Op::Out(6),
            Op::Out(8),
        ],
        true,
    );
    // A complete forwarded tool makes max-turns successful, without a checkpoint.
    replay(
        false,
        false,
        &[Op::Out(0), Op::Out(2), Op::Out(5), Op::Out(9)],
        true,
    );
    replay(false, true, &[Op::StdinFailed, Op::StdinFailed], true);
}
#[test]
fn coverage_distribution() {
    use proptest::strategy::ValueTree;
    let mut runner = proptest::test_runner::TestRunner::deterministic();
    for _ in 0..2000 {
        let (s, r, o) = history().new_tree(&mut runner).unwrap().current();
        replay(s, r, &o, true);
    }
    eprintln!(
        "lifecycle coverage [Keep, Discard, Commit, retry, killed]: {:?}",
        COUNTS
            .iter()
            .map(|n| n.load(Ordering::Relaxed))
            .collect::<Vec<_>>()
    );
}
