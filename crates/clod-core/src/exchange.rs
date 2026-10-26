//! I/O-free lifecycle. All response, retry and map decisions belong here.
use crate::{
    protocol::{self, Record},
    replay,
    request::{Content, Message, Request, TOOL_PREFIX},
    session::{self, Checkpoint, Final, MapEffect, Plan, SessionMap},
    spawn::{self, Config, SpawnSpec},
    sse::{error_envelope, SseEvent},
};
use alloc::{
    collections::BTreeSet,
    format,
    string::{String, ToString},
    vec,
    vec::Vec,
};
use serde_json::{json, Value};
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Spawn(SpawnSpec),
    WriteStdin(String),
    CloseStdin,
    Kill,
    StartSse,
    Emit(SseEvent),
    RespondJson(u16, Value),
    /// Close the HTTP body without releasing the session lock.
    EndResponse,
    Done(Final),
    /// Diagnostic only; the shell logs it without making a decision.
    Warning(String),
}
#[derive(Debug)]
pub struct Exchange {
    request: Request,
    config: Config,
    plan: Plan,
    key: Option<session::SessionKey>,
    retried: bool,
    stale: bool,
    committed: bool,
    sse: bool,
    gone: bool,
    done: bool,
    shutdown: Option<(u64, u64)>,
    killed: bool,
    caused: bool,
    effect: Option<MapEffect>,
    checkpoint_reason: Option<&'static str>,
    stdout_eof: bool,
    stderr_eof: bool,
    exit: Option<i32>,
    result: bool,
    started: u64,
    last_emit: u64,
    last_stream: u64,
    events: bool,
    generating: bool,
    boundary: bool,
    hold_ok: bool,
    hooks: Vec<String>,
    session_id: String,
    uuid: Option<String>,
    tool_uuid: Option<String>,
    bare_call: bool,
    prefixed_call: bool,
    multiple_messages: bool,
    last_assistant_has_tool: bool,
    done_count: u8,
    streamed: BTreeSet<String>,
    recorded: BTreeSet<String>,
    content: Vec<Value>,
    streamed_content: Vec<Value>,
    content_indexes: alloc::collections::BTreeMap<u64, usize>,
    input_fragments: alloc::collections::BTreeMap<String, String>,
    message: Value,
    terminal: Value,
    counter: u64,
}
impl Exchange {
    pub fn checkpoint_reason(&self) -> Option<&'static str> {
        self.checkpoint_reason.or(if self.result {
            None
        } else {
            Some("ResultNotSuccess")
        })
    }
    /// Planning borrows the map read-only. The shell must `touch` the key when
    /// acquiring its lock, before calling this constructor, and apply only `Done`.
    pub fn new(
        request: Request,
        map: &SessionMap,
        config: Config,
        now: u64,
    ) -> (Self, Vec<Action>) {
        let plan = session::plan_with_mode(&request, map, config.session_key);
        let key = session::key_with_mode(&request, config.session_key);
        let mut this = Self {
            request,
            config,
            plan,
            key,
            retried: false,
            stale: false,
            committed: false,
            sse: false,
            gone: false,
            done: false,
            shutdown: None,
            killed: false,
            caused: false,
            effect: None,
            checkpoint_reason: None,
            stdout_eof: false,
            stderr_eof: false,
            exit: None,
            result: false,
            started: now,
            last_emit: now,
            last_stream: now,
            events: false,
            generating: false,
            boundary: false,
            hold_ok: true,
            hooks: vec![],
            session_id: String::new(),
            uuid: None,
            tool_uuid: None,
            bare_call: false,
            prefixed_call: false,
            multiple_messages: false,
            last_assistant_has_tool: false,
            done_count: 0,
            streamed: BTreeSet::new(),
            recorded: BTreeSet::new(),
            content: vec![],
            streamed_content: vec![],
            content_indexes: alloc::collections::BTreeMap::new(),
            input_fragments: alloc::collections::BTreeMap::new(),
            message: Value::Null,
            terminal: json!({"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":0}}),
            counter: 0,
        };
        let invalid = this.request.tools.iter().find_map(|t| {
            let n = t["name"].as_str()?;
            (TOOL_PREFIX.len() + n.chars().count() > 64).then(|| n.to_string())
        });
        let actions = if let Some(name) = invalid {
            let mut a = this.error(
                400,
                "invalid_request_error",
                &format!("tool '{name}' exceeds 64 characters with the mcp__oc__ prefix"),
            );
            a.extend(this.finish());
            a
        } else {
            this.start()
        };
        (this, actions)
    }
    fn id(&mut self) -> String {
        self.counter += 1;
        format!("clod-{}", self.counter)
    }
    fn start(&mut self) -> Vec<Action> {
        let spec = spawn::spec(&self.request, &self.plan, &self.config);
        let prompt = spawn::system_prompt(&self.request, &self.plan);
        let id = self.id();
        let init = protocol::control(
            &id,
            json!({"subtype":"initialize","hooks":{"PreToolUse":[{"matcher":"mcp__oc__.*","hookCallbackIds":["hook_0"]}]},"sdkMcpServers":["oc"],"systemPrompt":[prompt]}),
        );
        let content = match &self.plan {
            Plan::Resume(_) if session::user_tail(&self.request.messages).1.len() == 1 => {
                serde_json::to_value(&self.request.messages.last().unwrap().content).unwrap()
            }
            Plan::Resume(_) => Value::Array(
                session::user_tail(&self.request.messages)
                    .1
                    .iter()
                    .flat_map(|m| match &m.content {
                        Content::Text(text) => vec![json!({"type":"text","text":text})],
                        Content::Blocks(blocks) => blocks.clone(),
                    })
                    .collect(),
            ),
            Plan::Fresh => Value::Array(replay::render(&self.request.messages)),
        };
        vec![Action::Spawn(spec),Action::WriteStdin(init),Action::WriteStdin(json!({"type":"user","message":{"role":"user","content":content},"parent_tool_use_id":null,"session_id":""}).to_string())]
    }
    fn error(&mut self, status: u16, kind: &str, text: &str) -> Vec<Action> {
        if self.gone {
            return vec![];
        }
        let body = error_envelope(kind, text);
        if self.committed {
            if self.sse {
                vec![
                    Action::Emit(SseEvent::new("error", body)),
                    Action::EndResponse,
                ]
            } else {
                vec![]
            }
        } else {
            self.committed = true;
            vec![Action::RespondJson(status, body), Action::EndResponse]
        }
    }
    fn close(&mut self, now: u64, delay: u64, caused: bool) -> Vec<Action> {
        if self.shutdown.is_some() {
            return vec![];
        }
        self.shutdown = Some((now, delay));
        self.caused |= caused;
        vec![Action::CloseStdin]
    }
    fn finish(&mut self) -> Vec<Action> {
        if self.done {
            return vec![];
        }
        // Bare-only fork ancestry was verified live; mixed bare/prefixed turns
        // could not be provoked live, so do not generalize that evidence to them.
        if self.bare_call && self.prefixed_call {
            self.effect = Some(MapEffect::Discard);
            self.checkpoint_reason = Some("MixedBareAndPrefixedToolCalls");
        }
        self.done_count += 1;
        debug_assert_eq!(self.done_count, 1, "exactly one Done per exchange");
        self.done = true;
        vec![Action::Done(Final {
            key: self.key,
            map_effect: self.effect.clone().unwrap_or(if self.stale {
                MapEffect::Discard
            } else {
                MapEffect::Keep
            }),
        })]
    }
    fn release(&mut self) -> Vec<Action> {
        self.hooks
            .drain(..)
            .map(|id| Action::WriteStdin(protocol::deny(&id)))
            .collect()
    }
    pub fn on_stdout_line(&mut self, line: &str, now: u64) -> Vec<Action> {
        if self.done {
            return vec![];
        }
        let record = match protocol::parse(line) {
            Ok(r) => r,
            Err(_) => return vec![],
        };
        match record {
            Record::ControlRequest(r) if self.shutdown.is_none() => {
                if r.request["subtype"] == "hook_callback" {
                    if self.generating {
                        self.hooks.push(r.request_id);
                        vec![]
                    } else {
                        vec![Action::WriteStdin(protocol::deny(&r.request_id))]
                    }
                } else if r.request["subtype"] == "mcp_message" {
                    let m = &r.request["message"];
                    let id = m.get("id").cloned().unwrap_or(json!(0));
                    let result = match m["method"].as_str() {
                        Some("initialize") => {
                            json!({"protocolVersion":"2025-11-25","capabilities":{"tools":{"listChanged":true}},"serverInfo":{"name":"oc","version":"1.0.0"}})
                        }
                        Some("tools/list") => {
                            let tools=self.request.tools.iter().map(|t|{let mut schema=t["input_schema"].clone(); if let Some(o)=schema.as_object_mut(){o.insert("$schema".into(),json!("http://json-schema.org/draft-07/schema#"));} json!({"name":t["name"],"description":t["description"],"inputSchema":schema,"execution":{"taskSupport":"forbidden"}})}).collect::<Vec<_>>();
                            json!({"tools":tools})
                        }
                        Some("tools/call") => {
                            json!({"isError":true,"content":[{"type":"text","text":"Tools must be executed by the client, not the CLI"}]})
                        }
                        _ => json!({}),
                    };
                    let mut actions = vec![Action::WriteStdin(protocol::response(
                        &r.request_id,
                        json!({"mcp_response":{"jsonrpc":"2.0","id":id,"result":result}}),
                    ))];
                    if m["method"] == "tools/call" {
                        actions.push(Action::Warning(
                            "CLI attempted tools/call; client owns tool execution".into(),
                        ));
                    }
                    actions
                } else {
                    vec![]
                }
            }
            Record::StreamEvent {
                mut event,
                session_id,
            } if self.shutdown.is_none() => {
                if let Some(id) = session_id {
                    self.session_id = id;
                }
                self.events = true;
                self.last_stream = now;
                let kind = event["type"].as_str().unwrap_or("").to_string();
                let mut actions = vec![];
                if self.request.stream && !self.sse {
                    self.sse = true;
                    self.committed = true;
                    actions.push(Action::StartSse);
                }
                if kind == "message_start" && !self.message.is_null() {
                    self.multiple_messages = true;
                    self.generating = false;
                    actions.extend(self.release());
                    actions.push(Action::Warning(
                        "CLI emitted multiple messages; suppressing subsequent content".into(),
                    ));
                    return actions;
                }
                if self.multiple_messages {
                    return actions;
                }
                match kind.as_str() {
                    "message_delta" => {
                        self.terminal = event;
                        self.generating = false;
                        self.boundary = true;
                        actions.extend(self.release());
                        return actions;
                    }
                    "message_stop" => {
                        self.generating = false;
                        self.boundary = true;
                        actions.extend(self.release());
                        return actions;
                    }
                    "message_start" => {
                        if !self.message.is_null() {
                            return actions;
                        }
                        self.message = event["message"].clone();
                        self.generating = true;
                    }
                    "content_block_start" => {
                        self.generating = true;
                        let block = &mut event["content_block"];
                        self.bare_call |= block["type"] == "tool_use"
                            && block["name"].as_str().is_some_and(|name| {
                                !name.starts_with(TOOL_PREFIX)
                                    && self
                                        .request
                                        .tools
                                        .iter()
                                        .any(|tool| tool["name"].as_str() == Some(name))
                            });
                        self.prefixed_call |= block["type"] == "tool_use"
                            && block["name"]
                                .as_str()
                                .is_some_and(|name| name.starts_with(TOOL_PREFIX));
                        strip(block);
                        if block["type"] == "tool_use" {
                            if let Some(id) = block["id"].as_str() {
                                self.streamed.insert(id.into());
                            }
                        }
                        let block = block.clone();
                        if let Some(index) = event["index"].as_u64() {
                            self.content_indexes
                                .insert(index, self.streamed_content.len());
                        }
                        self.streamed_content.push(block);
                    }
                    "content_block_delta" => {
                        let position = if let Some(index) = event["index"].as_u64() {
                            self.content_indexes.get(&index).copied()
                        } else {
                            self.streamed_content.len().checked_sub(1)
                        };
                        if let Some(block) = position.and_then(|p| self.streamed_content.get_mut(p))
                        {
                            let delta = &event["delta"];
                            let field = match delta["type"].as_str() {
                                Some("text_delta") => Some(("text", "text")),
                                Some("thinking_delta") => Some(("thinking", "thinking")),
                                Some("signature_delta") => Some(("signature", "signature")),
                                _ => None,
                            };
                            if let Some((target, source)) = field {
                                let mut text = block[target].as_str().unwrap_or("").to_string();
                                text.push_str(delta[source].as_str().unwrap_or(""));
                                block[target] = json!(text);
                            }
                            if delta["type"] == "input_json_delta" {
                                let id = block["id"].as_str().unwrap_or("").to_string();
                                self.input_fragments
                                    .entry(id)
                                    .or_default()
                                    .push_str(delta["partial_json"].as_str().unwrap_or(""));
                            }
                        }
                    }
                    _ => (),
                }
                if self.request.stream {
                    self.last_emit = now;
                    actions.push(Action::Emit(SseEvent::new(kind, event)));
                }
                actions
            }
            Record::Assistant(a) if self.shutdown.is_none() => {
                if self.multiple_messages {
                    return vec![];
                }
                if let Some(id) = a.session_id {
                    self.session_id = id;
                }
                self.uuid = a.uuid.clone();
                let mut tools = false;
                if let Some(blocks) = a.message["content"].as_array() {
                    for mut block in blocks.clone() {
                        if block["type"] == "tool_use"
                            && block["name"].as_str().is_some_and(|n| {
                                n.starts_with(TOOL_PREFIX)
                                    || self
                                        .request
                                        .tools
                                        .iter()
                                        .any(|tool| tool["name"].as_str() == Some(n))
                            })
                        {
                            tools = true;
                            self.prefixed_call |= block["name"]
                                .as_str()
                                .is_some_and(|n| n.starts_with(TOOL_PREFIX));
                            self.bare_call |= block["name"]
                                .as_str()
                                .is_some_and(|n| !n.starts_with(TOOL_PREFIX));
                            if let Some(id) = block["id"].as_str() {
                                self.recorded.insert(id.into());
                            }
                        }
                        strip(&mut block);
                        self.content.push(block);
                    }
                }
                self.last_assistant_has_tool = tools;
                if tools {
                    self.tool_uuid = a.uuid;
                }
                if !self.events {
                    self.generating = false;
                    self.boundary = true;
                    self.release()
                } else {
                    vec![]
                }
            }
            Record::System(v) => {
                if let Some(id) = v["session_id"].as_str() {
                    self.session_id = id.into();
                }
                vec![]
            }
            Record::Result(r)
                if self.shutdown.is_some() && !self.result && is_stale(&r.errors.join("\n")) =>
            {
                self.refusal(now)
            }
            Record::Result(r) if self.shutdown.is_none() => {
                self.result = true;
                self.checkpoint_reason = Some("ResultNotSuccess");
                if let Some(id) = r.session_id {
                    self.session_id = id;
                }
                let text = r.errors.join("\n");
                if is_stale(&text)
                    || (matches!(self.plan, Plan::Resume(_))
                        && (r.subtype == "invalid_request_error"
                            || text.to_ascii_lowercase().contains("invalid_request_error")))
                {
                    return self.refusal(now);
                }
                let tools = !self.streamed.is_empty() || !self.recorded.is_empty();
                if r.subtype == "success"
                    || (r.subtype == "error_max_turns" && tools && self.boundary)
                {
                    let uuid = if tools {
                        self.tool_uuid.clone()
                    } else {
                        self.uuid.clone()
                    };
                    let mut inputs_valid = true;
                    if !self.streamed_content.is_empty() {
                        for block in &mut self.streamed_content {
                            if let Some(fragment) = block["id"]
                                .as_str()
                                .and_then(|id| self.input_fragments.get(id))
                            {
                                // An empty delta is a valid no-argument call: retain the
                                // content_block_start input rather than parsing empty JSON.
                                if fragment.is_empty() {
                                    continue;
                                }
                                match serde_json::from_str::<Value>(fragment) {
                                    Ok(input) => block["input"] = input,
                                    Err(_) => inputs_valid = false,
                                }
                            }
                        }
                        self.content = self.streamed_content.clone();
                    }
                    self.checkpoint_reason = if self.multiple_messages {
                        Some("MultipleMessages")
                    } else if self.bare_call && self.prefixed_call {
                        Some("MixedBareAndPrefixedToolCalls")
                    } else if tools && !self.last_assistant_has_tool && self.tool_uuid.is_some() {
                        Some("TrailingContentAfterTool")
                    } else if !inputs_valid {
                        Some("InvalidToolInput")
                    } else if uuid.as_ref().is_none_or(|s| s.is_empty()) {
                        Some("MissingUuid")
                    } else if self.session_id.is_empty() {
                        Some("MissingSessionId")
                    } else if self.events && self.streamed != self.recorded {
                        Some("ToolIdsNotCovered")
                    } else if !self.hold_ok {
                        Some("HoldBackstop")
                    } else if !self.hooks.is_empty() {
                        Some("HeldHooks")
                    } else if tools && !self.boundary {
                        Some("MissingBoundary")
                    } else {
                        None
                    };
                    let valid = self.checkpoint_reason.is_none();
                    let mut history = self.request.messages.clone();
                    history.push(Message {
                        role: "assistant".into(),
                        content: Content::Blocks(self.content.clone()),
                    });
                    self.effect = Some(if valid {
                        MapEffect::Commit(Checkpoint {
                            session_id: self.session_id.clone(),
                            resume_uuid: uuid.unwrap(),
                            pending_tool_ids: if self.streamed.is_empty() {
                                self.recorded.iter().cloned().collect()
                            } else {
                                self.streamed.iter().cloned().collect()
                            },
                            prefix_hash: session::prefix_hash(&history),
                        })
                    } else {
                        MapEffect::Discard
                    });
                    if tools {
                        self.terminal["delta"]["stop_reason"] = json!("tool_use");
                    }
                    let mut actions = vec![];
                    if !self.gone {
                        if self.request.stream {
                            if !self.sse {
                                self.sse = true;
                                self.committed = true;
                                actions.push(Action::StartSse);
                            }
                            actions.push(Action::Emit(SseEvent::new(
                                "message_delta",
                                self.terminal.clone(),
                            )));
                            actions.push(Action::Emit(SseEvent::new(
                                "message_stop",
                                json!({"type":"message_stop"}),
                            )));
                        } else {
                            self.committed = true;
                            let mut m = self.message.clone();
                            if !m.is_object() {
                                m = json!({"type":"message","role":"assistant","model":self.request.model});
                            }
                            m["content"] = json!(self.content);
                            m["stop_reason"] = self.terminal["delta"]["stop_reason"].clone();
                            m["stop_sequence"] = Value::Null;
                            m["usage"] = r.usage;
                            actions.push(Action::RespondJson(200, m));
                        }
                    }
                    actions.push(Action::EndResponse);
                    actions.extend(self.close(now, 5000, true));
                    actions
                } else {
                    if self.bare_call {
                        self.effect = Some(MapEffect::Discard);
                        self.checkpoint_reason = Some("UnsuccessfulBareToolTurn");
                    }
                    let (status, kind, message) = classify(&text);
                    let mut a = self.error(status, kind, &message);
                    a.extend(self.close(now, 5000, true));
                    a
                }
            }
            _ => vec![],
        }
    }
    fn refusal(&mut self, now: u64) -> Vec<Action> {
        if self.stale && self.effect == Some(MapEffect::Discard) {
            return vec![];
        }
        self.stale = true;
        self.effect = Some(MapEffect::Discard);
        let retry = matches!(self.plan, Plan::Resume(_))
            && !self.retried
            && !self.committed
            && !self.gone
            && now.saturating_sub(self.started) < self.config.turn_timeout_ms;
        let mut a = if retry {
            vec![]
        } else {
            self.error(500, "api_error", "session could not be resumed")
        };
        a.extend(self.close(now, 5000, true));
        a
    }
    pub fn on_stderr_line(&mut self, line: &str, now: u64) -> Vec<Action> {
        if !self.done && !self.result && is_stale(line) {
            self.refusal(now)
        } else {
            vec![]
        }
    }
    pub fn on_stdout_eof(&mut self, now: u64) -> Vec<Action> {
        self.stdout_eof = true;
        self.reap(now)
    }
    pub fn on_stderr_eof(&mut self, now: u64) -> Vec<Action> {
        self.stderr_eof = true;
        self.reap(now)
    }
    /// Exit may be buffered defensively; classification waits for both EOFs.
    pub fn on_exit(&mut self, status: i32, now: u64) -> Vec<Action> {
        self.exit = Some(status);
        self.reap(now)
    }
    fn reap(&mut self, now: u64) -> Vec<Action> {
        if self.done || !self.stdout_eof || !self.stderr_eof || self.exit.is_none() {
            return vec![];
        }
        if self.stale
            && !self.retried
            && matches!(self.plan, Plan::Resume(_))
            && !self.committed
            && !self.gone
            && now.saturating_sub(self.started) < self.config.turn_timeout_ms
        {
            let request = self.request.clone();
            let config = self.config.clone();
            let counter = self.counter;
            let (mut next, _) = Self::new(request, &SessionMap::new(0), config, now);
            next.started = self.started;
            next.retried = true;
            next.stale = true;
            next.counter = counter;
            *self = next;
            return self.start();
        }
        let mut a = vec![];
        if !self.result {
            if self.effect.is_none()
                && matches!(self.plan, Plan::Resume(_))
                && !self.events
                && !self.caused
            {
                self.effect = Some(MapEffect::Discard);
            }
            if self.shutdown.is_none() {
                a.extend(self.error(500, "api_error", "CLI exited without a result"));
                a.extend(self.close(now, 5000, false));
            }
        }
        a.extend(self.finish());
        a
    }
    pub fn on_spawn_failed(&mut self, err: &str, _now: u64) -> Vec<Action> {
        if self.done {
            return vec![];
        }
        let mut a = self.error(500, "api_error", err);
        a.extend(self.finish());
        a
    }
    pub fn on_stdin_failed(&mut self, err: &str, now: u64) -> Vec<Action> {
        if self.done {
            return vec![];
        }
        if self.shutdown.is_some() {
            if !self.killed {
                self.killed = true;
                self.caused = true;
                return vec![Action::Kill];
            }
            return vec![];
        }
        let mut a = self.error(500, "api_error", err);
        a.extend(self.close(now, 5000, false));
        a
    }
    pub fn on_client_gone(&mut self, now: u64) -> Vec<Action> {
        if self.done {
            return vec![];
        }
        self.gone = true;
        if self.shutdown.is_some() {
            return vec![];
        }
        if self.effect.is_none() {
            self.effect = Some(if self.stale {
                MapEffect::Discard
            } else {
                MapEffect::Keep
            });
        }
        let id = self.id();
        let mut a = vec![Action::WriteStdin(protocol::interrupt(&id))];
        a.extend(self.close(now, 2000, true));
        a
    }
    pub fn on_tick(&mut self, now: u64) -> Vec<Action> {
        if self.done {
            return vec![];
        }
        let mut a = vec![];
        if let Some((start, delay)) = self.shutdown {
            if now.saturating_sub(start) >= delay && !self.killed {
                self.killed = true;
                self.caused = true;
                a.push(Action::Kill);
            }
            return a;
        }
        if now.saturating_sub(self.started) >= self.config.turn_timeout_ms {
            if self.effect.is_none() {
                self.effect = Some(if self.stale {
                    MapEffect::Discard
                } else {
                    MapEffect::Keep
                });
            }
            a.extend(self.error(500, "api_error", "turn timed out"));
            let id = self.id();
            a.push(Action::WriteStdin(protocol::interrupt(&id)));
            a.extend(self.close(now, 2000, true));
            return a;
        }
        if !self.hooks.is_empty()
            && now.saturating_sub(self.last_stream) >= self.config.hold_idle_ms
        {
            self.hold_ok = false;
            self.generating = false;
            a.extend(self.release());
        }
        if self.sse && !self.gone && now.saturating_sub(self.last_emit) >= self.config.ping_ms {
            self.last_emit = now;
            a.push(Action::Emit(SseEvent::new("ping", json!({"type":"ping"}))));
        }
        a
    }
}
fn strip(block: &mut Value) {
    if let Some(name) = block["name"]
        .as_str()
        .and_then(|n| n.strip_prefix(TOOL_PREFIX))
    {
        block["name"] = json!(name);
    }
}
fn is_stale(text: &str) -> bool {
    text.contains("No conversation found with session ID")
        || text.contains("No message found with message.uuid of")
}
fn classify(text: &str) -> (u16, &'static str, String) {
    let lower = text.to_ascii_lowercase();
    if lower.contains("rate limit") || lower.contains("usage limit") {
        (429, "rate_limit_error", text.into())
    } else if lower.contains("prompt is too long")
        || lower.contains("context") && lower.contains("overflow")
    {
        (400, "invalid_request_error", "prompt is too long".into())
    } else if lower.contains("authentication") || lower.contains("401") {
        (401, "authentication_error", text.into())
    } else {
        (500, "api_error", text.into())
    }
}
