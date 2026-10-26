//! I/O adapter for the decision-making core.
use axum::{
    body::{Body, Bytes},
    extract::{DefaultBodyLimit, Request as HttpRequest, State},
    http::{HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use clod_core::{
    exchange::{Action, Exchange},
    request::{self, Request},
    session::{self, SessionKey, SessionMap},
    spawn::Config,
    sse::error_envelope,
};
use std::{
    collections::{HashMap, VecDeque},
    convert::Infallible,
    env, io,
    path::PathBuf,
    process::Stdio,
    sync::{Arc, Mutex, Weak},
    time::{Duration, Instant},
};
use subtle::ConstantTimeEq;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines},
    process::{Child, ChildStderr, ChildStdout, Command},
    sync::{mpsc, oneshot, Mutex as AsyncMutex, Semaphore},
};
use tokio_stream::wrappers::UnboundedReceiverStream;

/// Measurement-only sink: never receives the CLI initialization/account payload.
fn measure(value: serde_json::Value) {
    use std::io::Write;
    if let Some(path) = env::var_os("CLOD_MEASURE_LOG") {
        match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            Ok(mut file) => {
                if let Err(error) = writeln!(file, "{value}") {
                    tracing::error!(%error, "measurement write failed");
                }
            }
            Err(error) => tracing::error!(%error, "measurement open failed"),
        }
    }
}
pub const TESTED_CLAUDE_VERSION: &str = "2.1.308";
#[derive(Clone)]
pub struct Settings {
    pub listen: String,
    pub api_key: Option<String>,
    pub core: Config,
    pub max_concurrent: usize,
    pub session_cap: usize,
    pub max_body_bytes: usize,
    pub allow_any_host: bool,
}
impl Settings {
    pub fn from_env() -> Result<Self, Box<dyn std::error::Error>> {
        let workdir = env::var_os("CLOD_WORKDIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                let state = env::var_os("XDG_STATE_HOME")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| {
                        PathBuf::from(env::var_os("HOME").unwrap_or_default()).join(".local/state")
                    });
                state.join("clod/workdir")
            });
        std::fs::create_dir_all(&workdir)?;
        if std::fs::read_dir(&workdir)?.next().is_some() {
            return Err("CLOD_WORKDIR must be empty (including no .git entry)".into());
        }
        let workdir = workdir.canonicalize()?;
        if workdir.ancestors().any(|dir| dir.join(".git").exists()) {
            return Err("CLOD_WORKDIR must not be inside a git repository".into());
        }
        let api_key = env::var("CLOD_API_KEY").ok();
        if api_key.as_ref().is_some_and(|key| key.is_empty()) {
            return Err("CLOD_API_KEY must not be empty".into());
        }
        let mut core = Config::new(workdir.to_str().ok_or("workdir must be UTF-8")?);
        core.program = env::var("CLOD_CLAUDE_BIN").unwrap_or_else(|_| "claude".into());
        if env::var_os("CLOD_SESSION_KEY").is_some() {
            return Err("CLOD_SESSION_KEY was removed; strict session keys are mandatory".into());
        }
        core.upstream_base_url = env::var("CLOD_UPSTREAM_BASE_URL").ok();
        if core
            .upstream_base_url
            .as_ref()
            .is_some_and(|url| !clod_core::security::upstream_url(url))
        {
            return Err("CLOD_UPSTREAM_BASE_URL must be an http loopback URL".into());
        }
        if core.upstream_base_url.is_some() {
            tracing::warn!(
                "measurement-only CLOD_UPSTREAM_BASE_URL is set; CLI traffic is redirected"
            );
        }
        // SAFETY: geteuid has no pointer arguments or preconditions.
        core.is_root = unsafe { libc::geteuid() == 0 };
        core.ping_ms = number("CLOD_PING_MS", core.ping_ms)?;
        core.hold_idle_ms = number("CLOD_HOLD_IDLE_MS", core.hold_idle_ms)?;
        core.turn_timeout_ms = number("CLOD_TURN_TIMEOUT_MS", core.turn_timeout_ms)?;
        let max_concurrent = number("CLOD_MAX_CONCURRENT", 8)?;
        if max_concurrent == 0 {
            return Err("CLOD_MAX_CONCURRENT must be positive".into());
        }
        Ok(Self {
            listen: env::var("CLOD_LISTEN").unwrap_or_else(|_| "127.0.0.1:8787".into()),
            api_key,
            max_body_bytes: number("CLOD_MAX_BODY_BYTES", 256 * 1024 * 1024)?,
            allow_any_host: env::var("CLOD_ALLOW_ANY_HOST").as_deref() == Ok("1"),
            core,
            max_concurrent,
            session_cap: number("CLOD_SESSION_CAP", 256)?,
        })
    }
    pub async fn check_version(&self) {
        let mut command = Command::new(&self.core.program);
        command.arg("--version").kill_on_drop(true);
        match tokio::time::timeout(Duration::from_secs(5), command.output()).await {
            Ok(Ok(output)) if output.status.success() => {
                let version = String::from_utf8_lossy(&output.stdout);
                if version.split_whitespace().next() != Some(TESTED_CLAUDE_VERSION) {
                    tracing::warn!(actual = %version.trim(), expected = TESTED_CLAUDE_VERSION, "untested Claude CLI version");
                }
            }
            other => tracing::warn!(result = ?other, "could not check Claude CLI version"),
        }
    }
}
fn number<T: std::str::FromStr>(name: &str, default: T) -> Result<T, Box<dyn std::error::Error>>
where
    T::Err: std::error::Error + 'static,
{
    match env::var(name) {
        Ok(value) => Ok(value.parse()?),
        Err(env::VarError::NotPresent) => Ok(default),
        Err(e) => Err(e.into()),
    }
}
struct Shared {
    settings: Settings,
    map: Mutex<SessionMap>,
    locks: Mutex<HashMap<SessionKey, Weak<AsyncMutex<()>>>>,
    permits: Arc<Semaphore>,
    clock: Instant,
}
impl Shared {
    fn now(&self) -> u64 {
        self.clock.elapsed().as_millis() as u64
    }
}
pub fn router(settings: Settings) -> Router {
    let shared = Arc::new(Shared {
        map: Mutex::new(SessionMap::new(settings.session_cap)),
        locks: Mutex::new(HashMap::new()),
        permits: Arc::new(Semaphore::new(settings.max_concurrent)),
        settings,
        clock: Instant::now(),
    });
    Router::new()
        .route("/v1/messages", post(messages))
        .route("/health", get(|| async { "ok" }))
        .route("/v1/models", get(|| async { Json(serde_json::json!({"data":[{"id":"claude-sonnet-4-6","type":"model","display_name":"Claude Sonnet 4.6"}],"has_more":false,"first_id":"claude-sonnet-4-6","last_id":"claude-sonnet-4-6"})) }))
        .fallback(|| async { error(404, "not_found_error", "Not found") })
        .method_not_allowed_fallback(|| async { error(404, "not_found_error", "Not found") })
        .layer(DefaultBodyLimit::max(shared.settings.max_body_bytes))
        .layer(middleware::from_fn_with_state(shared.clone(), auth))
        .with_state(shared)
}
fn error(status: u16, kind: &str, message: &str) -> Response {
    (
        StatusCode::from_u16(status).unwrap(),
        Json(error_envelope(kind, message)),
    )
        .into_response()
}
async fn auth(State(shared): State<Arc<Shared>>, request: HttpRequest, next: Next) -> Response {
    let headers = request.headers();
    if !clod_core::security::inbound_allowed(
        headers.contains_key("origin"),
        headers.get("host").and_then(|v| v.to_str().ok()),
        shared.settings.allow_any_host,
    ) {
        return error(
            403,
            "permission_error",
            "Origin is forbidden and Host must be loopback",
        );
    }
    if let Some(expected) = &shared.settings.api_key {
        let key = headers.get("x-api-key").map(|v| v.as_bytes());
        let bearer = headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split_once(' '))
            .and_then(|(scheme, token)| scheme.eq_ignore_ascii_case("Bearer").then_some(token));
        let valid = (key.unwrap_or_default().ct_eq(expected.as_bytes())
            & subtle::Choice::from(u8::from(key.is_some())))
            | (bearer
                .unwrap_or_default()
                .as_bytes()
                .ct_eq(expected.as_bytes())
                & subtle::Choice::from(u8::from(bearer.is_some())));
        if !bool::from(valid) {
            return error(401, "authentication_error", "Invalid API key");
        }
    }
    next.run(request).await
}
async fn messages(
    State(shared): State<Arc<Shared>>,
    headers: HeaderMap,
    body: Result<Bytes, axum::extract::rejection::BytesRejection>,
) -> Response {
    let body = match body {
        Ok(body) => body,
        Err(error) => return self::error(400, "invalid_request_error", &error.to_string()),
    };
    let headers: Vec<_> = headers
        .iter()
        .filter_map(|(k, v)| Some((k.as_str(), v.to_str().ok()?)))
        .collect();
    let request = match request::parse(&body, &headers) {
        Ok(request) => request,
        Err(e) => return error(400, "invalid_request_error", &e.to_string()),
    };
    let (response_tx, response_rx) = oneshot::channel();
    let (body_tx, body_rx) = mpsc::unbounded_channel();
    tokio::spawn(run(shared, request, response_tx, body_tx, body_rx));
    response_rx
        .await
        .unwrap_or_else(|_| error(500, "api_error", "Request runner stopped"))
}
type BodySender = mpsc::UnboundedSender<Result<Bytes, Infallible>>;
struct Process {
    child: Child,
    group: i32,
    stdin: Option<mpsc::UnboundedSender<String>>,
    stdin_errors: mpsc::UnboundedReceiver<String>,
    writer: tokio::task::JoinHandle<()>,
    stdout: Lines<BufReader<ChildStdout>>,
    stderr: Lines<BufReader<ChildStderr>>,
    stdout_eof: bool,
    stderr_eof: bool,
    exit: Option<i32>,
    killed: Option<Instant>,
}
impl Process {
    fn spawn(spec: clod_core::spawn::SpawnSpec) -> io::Result<Self> {
        let mut command = Command::new(spec.program);
        command
            .args(spec.args)
            .current_dir(spec.working_directory)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .process_group(0);
        for (name, _) in env::vars_os() {
            if name
                .to_str()
                .is_some_and(clod_core::security::scrub_child_env)
            {
                command.env_remove(name);
            }
        }
        // Explicit core settings override inherited-variable scrubbing.
        command.envs(spec.env_set.clone());
        for name in spec.env_remove {
            command.env_remove(name);
        }
        let mut child = command.spawn()?;
        let stdout = BufReader::new(child.stdout.take().unwrap()).lines();
        let stderr = BufReader::new(child.stderr.take().unwrap()).lines();
        let mut child_stdin = child.stdin.take().unwrap();
        let (stdin, mut writes) = mpsc::unbounded_channel::<String>();
        let (errors, stdin_errors) = mpsc::unbounded_channel();
        let writer = tokio::spawn(async move {
            while let Some(line) = writes.recv().await {
                if let Err(error) = child_stdin.write_all(line.as_bytes()).await {
                    let _ = errors.send(error.to_string());
                    break;
                }
            }
        });
        Ok(Self {
            group: child.id().expect("new child has a PID") as i32,
            child,
            stdin: Some(stdin),
            stdin_errors,
            writer,
            stdout,
            stderr,
            stdout_eof: false,
            stderr_eof: false,
            exit: None,
            killed: None,
        })
    }
    fn kill(&mut self) {
        // SAFETY: the child created this process group. Retain the ID after wait()
        // so descendants holding a pipe are signalled even if the leader exited.
        unsafe {
            libc::killpg(self.group, libc::SIGKILL);
        }
        self.killed.get_or_insert_with(Instant::now);
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        if self.child.id().is_some() {
            // Until wait reaps our child, its PID (even as a zombie) reserves the
            // group ID. After reaping, skip this to avoid signalling a reused PGID.
            self.kill();
        }
        self.writer.abort();
    }
}
async fn run(
    shared: Arc<Shared>,
    request: Request,
    response: oneshot::Sender<Response>,
    body: BodySender,
    receiver: mpsc::UnboundedReceiver<Result<Bytes, Infallible>>,
) {
    let key = session::key_with_mode(&request, shared.settings.core.session_key);
    let lock = key.map(|key| {
        let mut locks = shared.locks.lock().unwrap();
        let lock = locks
            .get(&key)
            .and_then(Weak::upgrade)
            .unwrap_or_else(|| Arc::new(AsyncMutex::new(())));
        locks.insert(key, Arc::downgrade(&lock));
        lock
    });
    let guard = match &lock {
        Some(lock) => Some(lock.lock().await),
        None => None,
    };
    let _permit = shared.permits.clone().acquire_owned().await.unwrap();
    let (mut exchange, actions) = {
        let mut map = shared.map.lock().unwrap();
        if let Some(key) = key {
            map.touch(&key);
        }
        let reason = session::fresh_reason(&request, &map, shared.settings.core.session_key);
        tracing::debug!(?key, ?reason, "session planning");
        measure(serde_json::json!({"kind":"decision","key":key,"reason":format!("{reason:?}")}));
        Exchange::new(request, &map, shared.settings.core.clone(), shared.now())
    };
    static NEXT_MEASUREMENT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let measurement_id = NEXT_MEASUREMENT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut cli_attempt = 0_u64;
    let mut actions: VecDeque<_> = actions.into();
    let mut process: Option<Process> = None;
    let mut body = Some(body);
    let mut response = Some(response);
    let mut receiver = Some(receiver);
    let mut gone = false;
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    loop {
        while let Some(action) = actions.pop_front() {
            let now = shared.now();
            let observations = match action {
                Action::Spawn(spec) => {
                    cli_attempt += 1;
                    let source = spec
                        .args
                        .windows(2)
                        .find(|w| w[0] == "--resume")
                        .map(|w| w[1].clone());
                    measure(
                        serde_json::json!({"kind":"spawn","request":measurement_id,"attempt":cli_attempt,"plan":if source.is_some() {"resume"} else {"fresh"},"key_mode":format!("{:?}",shared.settings.core.session_key),"source_session_id":source}),
                    );
                    match Process::spawn(spec) {
                        Ok(child) => {
                            process = Some(child);
                            vec![]
                        }
                        Err(e) => exchange.on_spawn_failed(&e.to_string(), now),
                    }
                }
                Action::WriteStdin(mut line) => {
                    if !line.ends_with('\n') {
                        line.push('\n');
                    }
                    match process.as_ref().and_then(|p| p.stdin.as_ref()) {
                        Some(stdin) if stdin.send(line).is_ok() => vec![],
                        _ => exchange.on_stdin_failed("stdin closed", now),
                    }
                }
                Action::CloseStdin => {
                    if let Some(p) = &mut process {
                        p.stdin.take();
                    }
                    vec![]
                }
                Action::Kill => {
                    if let Some(p) = &mut process {
                        p.kill();
                    }
                    vec![]
                }
                Action::Warning(message) => {
                    tracing::warn!(%message);
                    vec![]
                }
                Action::StartSse => {
                    if let (Some(tx), Some(rx)) = (response.take(), receiver.take()) {
                        let reply = Response::builder()
                            .header("content-type", "text/event-stream")
                            .header("cache-control", "no-cache")
                            .body(Body::from_stream(UnboundedReceiverStream::new(rx)))
                            .unwrap();
                        if tx.send(reply).is_err() && !gone {
                            gone = true;
                            actions.extend(exchange.on_client_gone(now));
                        }
                    }
                    vec![]
                }
                Action::Emit(event) => {
                    if body
                        .as_ref()
                        .is_some_and(|body| body.send(Ok(Bytes::from(event.encode()))).is_err())
                        && !gone
                    {
                        gone = true;
                        exchange.on_client_gone(now)
                    } else {
                        vec![]
                    }
                }
                Action::RespondJson(status, value) => {
                    if let (Some(tx), Some(rx)) = (response.take(), receiver.take()) {
                        let reply = Response::builder()
                            .status(status)
                            .header("content-type", "application/json")
                            .body(Body::from_stream(UnboundedReceiverStream::new(rx)))
                            .unwrap();
                        let failed = tx.send(reply).is_err()
                            | body.as_ref().is_some_and(|body| {
                                body.send(Ok(Bytes::from(value.to_string()))).is_err()
                            });
                        if failed && !gone {
                            gone = true;
                            actions.extend(exchange.on_client_gone(now));
                        }
                    }
                    vec![]
                }
                Action::EndResponse => {
                    body.take();
                    vec![]
                }
                Action::Done(final_state) => {
                    let reason = exchange.checkpoint_reason();
                    tracing::debug!(effect=?final_state.map_effect, ?reason, "session effect");
                    measure(
                        serde_json::json!({"kind":"effect","request":measurement_id,"key":final_state.key,"effect":format!("{:?}",final_state.map_effect),"reason":reason}),
                    );
                    session::apply(&mut shared.map.lock().unwrap(), final_state);
                    drop(guard);
                    drop(lock);
                    if let Some(key) = key {
                        let mut locks = shared.locks.lock().unwrap();
                        if locks
                            .get(&key)
                            .is_some_and(|entry| entry.strong_count() == 0)
                        {
                            locks.remove(&key);
                        }
                    }
                    return;
                }
            };
            // Preserve the order of actions already emitted by the core.
            actions.extend(observations);
        }
        if let Some(p) = &mut process {
            if p.stdout_eof && p.stderr_eof {
                if let Some(code) = p.exit.take() {
                    process = None;
                    actions.extend(exchange.on_exit(code, shared.now()));
                    continue;
                }
            }
        }
        let p = process.as_mut().expect("core must spawn or finish");
        tokio::select! {
            _ = async { if let Some(tx) = &mut response { tx.closed().await } else if let Some(body) = &body { body.closed().await } else { core::future::pending::<()>().await } }, if !gone => {
                gone = true; actions.extend(exchange.on_client_gone(shared.now()));
            }
            _ = tick.tick() => {
                let now = shared.now();
                if p.killed.is_some_and(|at| at.elapsed() >= Duration::from_secs(2)) {
                    if !p.stdout_eof { p.stdout_eof = true; actions.extend(exchange.on_stdout_eof(now)); }
                    if !p.stderr_eof { p.stderr_eof = true; actions.extend(exchange.on_stderr_eof(now)); }
                }
                actions.extend(exchange.on_tick(now));
            }
            line = p.stdout.next_line(), if !p.stdout_eof => {
                match line {
                    Ok(Some(line)) => {
                        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) {
                            let event = if value["type"] == "stream_event" { &value["event"] } else { &value };
                            let usage = if event["type"] == "message_start" { event["message"].get("usage") } else if event["type"] == "message_delta" || event["type"] == "result" { event.get("usage") } else { None };
                            if let Some(usage) = usage {
                                measure(serde_json::json!({"kind":"usage","request":measurement_id,"attempt":cli_attempt,"event":event["type"],"session_id":value.get("session_id"),"usage":usage}));
                            }
                        }
                        actions.extend(exchange.on_stdout_line(&line, shared.now()));
                    }
                    other => { if let Err(e) = other { tracing::warn!(%e, "stdout read failed"); } p.stdout_eof = true; actions.extend(exchange.on_stdout_eof(shared.now())); }
                }
            }
            line = p.stderr.next_line(), if !p.stderr_eof => {
                match line {
                    Ok(Some(line)) => { tracing::debug!(%line, "Claude stderr"); actions.extend(exchange.on_stderr_line(&line, shared.now())); }
                    other => { if let Err(e) = other { tracing::warn!(%e, "stderr read failed"); } p.stderr_eof = true; actions.extend(exchange.on_stderr_eof(shared.now())); }
                }
            }
            Some(error) = p.stdin_errors.recv() => {
                actions.extend(exchange.on_stdin_failed(&error, shared.now()));
            }
            status = p.child.wait(), if p.exit.is_none() => {
                p.exit = Some(status.ok().and_then(|s| s.code()).unwrap_or(-1));
                p.kill();
            }
        }
    }
}
