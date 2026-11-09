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
    if let Some(path) = env::var_os("CLOD_MEASURE_LOG") {
        if let Err(error) = append_record(std::path::Path::new(&path), &value) {
            tracing::error!(%error, "measurement write failed");
        }
    }
}
/// Append one JSON line. The record is serialized in full first and written
/// with a single `write_all` on an append-mode file while holding a
/// process-wide lock, so concurrent requests never interleave partial lines.
/// The file is reopened per record so external truncation or rotation of the
/// path behaves as before.
fn append_record(path: &std::path::Path, value: &serde_json::Value) -> io::Result<()> {
    use std::io::Write;
    static LOCK: Mutex<()> = Mutex::new(());
    let mut line = serde_json::to_vec(value)?;
    line.push(b'\n');
    let _guard = LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?
        .write_all(&line)
}
/// Short prefix of the session key hash for correlating log lines. The full
/// key is only written to measurement records.
fn epoch_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn key_label(key: &SessionKey) -> String {
    key[..6].iter().map(|byte| format!("{byte:02x}")).collect()
}
fn fresh_label(reason: Option<&session::FreshReason>) -> &'static str {
    use session::FreshReason::*;
    match reason {
        None => "none",
        Some(NoKey) => "no_key",
        Some(NoCheckpoint) => "no_checkpoint",
        Some(InvalidCheckpoint) => "invalid_checkpoint",
        Some(NotSingleUserTail) => "not_single_user_tail",
        Some(PrefixMismatch { .. }) => "prefix_mismatch",
        Some(PendingMismatch) => "pending_mismatch",
        Some(TextWithPending) => "text_with_pending",
    }
}
fn effect_label(effect: &session::MapEffect) -> &'static str {
    match effect {
        session::MapEffect::Commit(_) => "commit",
        session::MapEffect::Keep => "keep",
        session::MapEffect::Discard => "discard",
    }
}
/// Numeric token counts from a CLI usage object; absent fields count as zero.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct UsageCounts {
    input: u64,
    output: u64,
    cache_read: u64,
    cache_creation: u64,
}
impl UsageCounts {
    fn from_json(usage: &serde_json::Value) -> Self {
        let count = |name: &str| usage.get(name).and_then(|v| v.as_u64()).unwrap_or(0);
        Self {
            input: count("input_tokens"),
            output: count("output_tokens"),
            cache_read: count("cache_read_input_tokens"),
            cache_creation: count("cache_creation_input_tokens"),
        }
    }
}
pub const TESTED_CLAUDE_VERSION: &str = "2.1.319";
#[derive(Clone)]
pub struct Settings {
    pub listen: String,
    pub api_key: Option<String>,
    pub core: Config,
    pub max_concurrent: usize,
    pub session_cap: usize,
    pub max_body_bytes: usize,
    pub allow_any_host: bool,
    pub weekly_breaker: bool,
    pub weekly_breaker_threshold: clod_core::quota::WeeklyThreshold,
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
        core.align_tool_names = match env::var_os("CLOD_ALIGN_TOOL_NAMES") {
            None => false,
            Some(value) if value == "1" => true,
            Some(_) => {
                return Err(
                    "CLOD_ALIGN_TOOL_NAMES must be 1 when set (unset disables the experiment)"
                        .into(),
                )
            }
        };
        if core.align_tool_names {
            tracing::warn!("experimental transport tool-name alignment enabled; use a new process with an empty session store");
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
            weekly_breaker_threshold: {
                let percent: f64 = number("CLOD_WEEKLY_BREAKER_PERCENT", 1.0)
                    .map_err(|_| "CLOD_WEEKLY_BREAKER_PERCENT must be a finite number greater than 0 and at most 100")?;
                clod_core::quota::WeeklyThreshold::new(percent / 100.0)
                    .map_err(|_| "CLOD_WEEKLY_BREAKER_PERCENT must be a finite number greater than 0 and at most 100")?
            },
            weekly_breaker: match env::var_os("CLOD_WEEKLY_BREAKER") {
                None => false,
                Some(value) if value == "1" => true,
                Some(_) => {
                    return Err("CLOD_WEEKLY_BREAKER must be 1 when set (unset disables it)".into())
                }
            },
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
/// Shell process ownership shared with the synchronous shutdown path.
#[derive(Clone, Default)]
pub struct ProcessGroups(Arc<Mutex<GroupState>>);
#[derive(Default)]
struct GroupState {
    stopping: bool,
    live: std::collections::HashSet<i32>,
}
impl ProcessGroups {
    /// The registry stays usable after a panic elsewhere: cleanup in Drop and
    /// shutdown must still run rather than panic on a poisoned lock.
    fn state(&self) -> std::sync::MutexGuard<'_, GroupState> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    /// Close spawning and kill groups before waiting for runtime workers. No
    /// filesystem, tracing or other potentially blocking I/O runs under this lock.
    pub fn shutdown(&self) {
        let mut groups = self.state();
        groups.stopping = true;
        for &group in &groups.live {
            // SAFETY: spawn/reap/drop serialize with this lock; these leaders
            // remain unreaped and therefore reserve their process-group IDs.
            unsafe {
                libc::killpg(group, libc::SIGKILL);
            }
        }
    }
}
struct Shared {
    groups: ProcessGroups,
    settings: Settings,
    map: Mutex<SessionMap>,
    locks: Mutex<HashMap<SessionKey, Weak<AsyncMutex<()>>>>,
    permits: Arc<Semaphore>,
    clock: Instant,
    quota: Mutex<clod_core::quota::WeeklyBreaker>,
}
impl Shared {
    fn now(&self) -> u64 {
        self.clock.elapsed().as_millis() as u64
    }
}
pub fn router(settings: Settings) -> Router {
    router_with_groups(settings, ProcessGroups::default())
}
pub fn router_with_groups(settings: Settings, groups: ProcessGroups) -> Router {
    let shared = Arc::new(Shared {
        groups,
        map: Mutex::new(SessionMap::new(settings.session_cap)),
        locks: Mutex::new(HashMap::new()),
        permits: Arc::new(Semaphore::new(settings.max_concurrent)),
        quota: Mutex::new(clod_core::quota::WeeklyBreaker::new(
            settings.weekly_breaker_threshold,
        )),
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
    // One ID per accepted request: the `request` field in measurement records
    // and the `id` of the log span, so both can be correlated.
    static NEXT_REQUEST: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let id = NEXT_REQUEST.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let span = tracing::info_span!("request", id);
    tokio::spawn(tracing::Instrument::instrument(
        run(shared, request, id, response_tx, body_tx, body_rx),
        span,
    ));
    response_rx
        .await
        .unwrap_or_else(|_| error(500, "api_error", "Request runner stopped"))
}
type BodySender = mpsc::UnboundedSender<Result<Bytes, Infallible>>;
struct Process {
    groups: ProcessGroups,
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
    #[cfg(test)]
    group_signals: Option<usize>,
}
impl Process {
    fn spawn(spec: clod_core::spawn::SpawnSpec, groups: ProcessGroups) -> io::Result<Self> {
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
        let mut registered = groups.state();
        if registered.stopping {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "server stopping",
            ));
        }
        let mut child = command.spawn()?;
        registered
            .live
            .insert(child.id().expect("new child has a PID") as i32);
        drop(registered);
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
            groups,
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
            #[cfg(test)]
            group_signals: None,
        })
    }
    async fn wait(
        child: &mut Child,
        group: i32,
        groups: &ProcessGroups,
    ) -> io::Result<std::process::ExitStatus> {
        std::future::poll_fn(|cx| {
            let mut registered = groups.state();
            let result = std::future::Future::poll(std::pin::pin!(child.wait()).as_mut(), cx);
            if result.is_ready() && child.id().is_none() {
                // No await or external I/O between reap, removal and the one
                // immediate descendant kill. Shutdown cannot see a reaped PGID.
                registered.live.remove(&group);
                // SAFETY: immediate post-reap cleanup, serialized with shutdown.
                unsafe {
                    libc::killpg(group, libc::SIGKILL);
                }
            }
            result
        })
        .await
    }
    fn kill(&mut self) {
        if self.child.id().is_some() {
            self.kill_group();
        }
    }
    // Only for an unreaped child: call it through kill(). The one post-reap
    // group kill happens inside Process::wait, under the registry lock.
    fn kill_group(&mut self) {
        // SAFETY: this is our unreaped child's group, or immediate reap cleanup.
        #[cfg(test)]
        if let Some(count) = &mut self.group_signals {
            *count += 1;
            self.killed.get_or_insert_with(Instant::now);
            return;
        }
        unsafe {
            libc::killpg(self.group, libc::SIGKILL);
        }
        self.killed.get_or_insert_with(Instant::now);
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        let groups = self.groups.clone();
        let mut registered = groups.state();
        registered.live.remove(&self.group);
        if self.child.id().is_some() {
            // Until wait reaps our child, its PID (even as a zombie) reserves the
            // group ID. After reaping, skip this to avoid signalling a reused PGID.
            self.kill();
        }
        self.writer.abort();
    }
}
fn log_rate_limit(
    request: u64,
    source: &'static str,
    http_status: u16,
    snapshot: clod_core::quota::Snapshot,
) {
    // Only bounded labels and numeric quota data; never CLI error text.
    let record = serde_json::json!({
        "kind": "rate_limit", "request": request, "source": source,
        "http_status": http_status, "semantic_status": 429,
        "observed_epoch": epoch_seconds(), "breaker": snapshot,
    });
    tracing::warn!(request, source, http_status, semantic_status = 429,
        breaker_enabled = snapshot.enabled, breaker_state = snapshot.state,
        details = %record, "request rate limited");
    measure(record);
}

async fn run(
    shared: Arc<Shared>,
    request: Request,
    measurement_id: u64,
    response: oneshot::Sender<Response>,
    body: BodySender,
    receiver: mpsc::UnboundedReceiver<Result<Bytes, Infallible>>,
) {
    let started = Instant::now();
    // Shape only: never log prompt, tool or account contents.
    tracing::info!(
        model = %request.model,
        stream = request.stream,
        messages = request.messages.len(),
        tools = request.tools.len(),
        "request received"
    );
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
    // Recheck after both waits: queued requests must see quota observed by
    // earlier runners. No exchange or checkpoint mutation has happened yet.
    if shared.settings.weekly_breaker {
        let snapshot = shared
            .quota
            .lock()
            .unwrap()
            .snapshot(true, shared.now(), epoch_seconds());
        if let Some(retry) = snapshot.earliest_recheck_ms {
            log_rate_limit(measurement_id, "local_weekly_breaker", 429, snapshot);
            let message = format!("Weekly subscription usage increased by more than {} percentage points within an hour; recheck after {} seconds", shared.settings.weekly_breaker_threshold.percent(), retry.div_ceil(1000));
            let _ = response.send(error(429, "rate_limit_error", &message));
            return;
        }
    }
    let (mut exchange, actions) = {
        let mut map = shared.map.lock().unwrap();
        if let Some(key) = key {
            map.touch(&key);
        }
        let reason = session::fresh_reason(&request, &map, shared.settings.core.session_key);
        tracing::info!(
            session = key.as_ref().map(key_label).as_deref().unwrap_or("none"),
            fresh_reason = fresh_label(reason.as_ref()),
            queued_ms = started.elapsed().as_millis() as u64,
            "session planned"
        );
        measure(serde_json::json!({"kind":"decision","key":key,"reason":format!("{reason:?}")}));
        Exchange::new(request, &map, shared.settings.core.clone(), shared.now())
    };
    let mut cli_attempt = 0_u64;
    // Usage from the latest attempt's `result` record: the CLI's final total
    // for the turn. Streaming usage records are partial and never summed.
    let mut result_usage: Option<UsageCounts> = None;
    let mut http_status: Option<u16> = None;
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
                Action::RateLimited { http_status } => {
                    let snapshot = shared.quota.lock().unwrap().snapshot(
                        shared.settings.weekly_breaker,
                        now,
                        epoch_seconds(),
                    );
                    log_rate_limit(measurement_id, "cli_rate_limit", http_status, snapshot);
                    vec![]
                }
                Action::Quota(observation, observed_ms) => {
                    let epoch = epoch_seconds();
                    shared
                        .quota
                        .lock()
                        .unwrap()
                        .observe(&observation, observed_ms, epoch);
                    for (window, value) in [
                        ("five_hour", observation.five_hour),
                        ("seven_day", observation.seven_day),
                    ] {
                        if let Some(value) = value {
                            let percent = value.utilization * 100.0;
                            tracing::info!(
                                source = "cli_rate_limit_event",
                                window,
                                utilization = value.utilization,
                                percent,
                                resets_at = value.resets_at,
                                observed_epoch = epoch,
                                observed_ms,
                                "subscription quota observed"
                            );
                            measure(
                                serde_json::json!({"kind":"quota","source":"cli_rate_limit_event","window":window,"utilization":value.utilization,"percent":percent,"resets_at":value.resets_at,"observed_epoch":epoch,"observed_ms":observed_ms}),
                            );
                        }
                    }
                    vec![]
                }
                Action::Spawn(spec) => {
                    cli_attempt += 1;
                    let source = spec
                        .args
                        .windows(2)
                        .find(|w| w[0] == "--resume")
                        .map(|w| w[1].clone());
                    let plan = if source.is_some() { "resume" } else { "fresh" };
                    result_usage = None;
                    tracing::info!(
                        attempt = cli_attempt,
                        plan,
                        resume_session = source.as_deref(),
                        "spawning CLI"
                    );
                    measure(
                        serde_json::json!({"kind":"spawn","request":measurement_id,"attempt":cli_attempt,"plan":plan,"key_mode":format!("{:?}",shared.settings.core.session_key),"source_session_id":source}),
                    );
                    match Process::spawn(spec, shared.groups.clone()) {
                        Ok(child) => {
                            process = Some(child);
                            #[cfg(feature = "test-support")]
                            if let Some(marker) = env::var_os("CLOD_TEST_BLOCK_WORKER") {
                                // Test-only synchronous stall while this runner
                                // owns a registered CLI. Runtime task cancellation
                                // cannot drop it; shutdown must kill via registry.
                                std::fs::write(marker, b"blocked").unwrap();
                                loop {
                                    std::thread::park();
                                }
                            }
                            vec![]
                        }
                        Err(e) => {
                            tracing::warn!(attempt = cli_attempt, error = %e, "CLI spawn failed");
                            exchange.on_spawn_failed(&e.to_string(), now)
                        }
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
                        http_status = Some(200);
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
                        http_status = Some(status);
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
                    measure(
                        serde_json::json!({"kind":"effect","request":measurement_id,"key":final_state.key,"effect":format!("{:?}",final_state.map_effect),"reason":reason}),
                    );
                    // Logged after the last measurement record for this request.
                    let usage = result_usage;
                    tracing::info!(
                        status = http_status,
                        attempts = cli_attempt,
                        effect = effect_label(&final_state.map_effect),
                        checkpoint_reason = reason,
                        client_gone = gone,
                        elapsed_ms = started.elapsed().as_millis() as u64,
                        input_tokens = usage.map(|u| u.input),
                        output_tokens = usage.map(|u| u.output),
                        cache_read_input_tokens = usage.map(|u| u.cache_read),
                        cache_creation_input_tokens = usage.map(|u| u.cache_creation),
                        "request finished"
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
                    tracing::info!(attempt = cli_attempt, code, "CLI exited");
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
                                if event["type"] == "result" {
                                    result_usage = Some(UsageCounts::from_json(usage));
                                }
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
            status = Process::wait(&mut p.child, p.group, &p.groups), if p.exit.is_none() => {
                p.exit = Some(status.ok().and_then(|s| s.code()).unwrap_or(-1));
                p.killed.get_or_insert_with(Instant::now);
            }
        }
    }
}

#[cfg(test)]
mod logging_tests {
    use super::*;

    #[test]
    fn concurrent_measurement_records_never_interleave() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("measure.ndjson");
        // Large records span many formatter writes; each must land whole.
        let threads: Vec<_> = (0..8)
            .map(|thread| {
                let path = path.clone();
                std::thread::spawn(move || {
                    for index in 0..50 {
                        let value = serde_json::json!({"kind":"usage","thread":thread,"index":index,"pad":"x".repeat(8192)});
                        append_record(&path, &value).unwrap();
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<_> = text.lines().collect();
        assert_eq!(lines.len(), 400);
        for line in lines {
            let value: serde_json::Value = serde_json::from_str(line).unwrap();
            assert_eq!(value["pad"].as_str().unwrap().len(), 8192);
        }
    }

    #[test]
    fn record_format_matches_display() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("measure.ndjson");
        let value = serde_json::json!({"kind":"decision","key":[1,2],"reason":"None"});
        append_record(&path, &value).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            format!("{value}\n")
        );
    }

    #[test]
    fn usage_counts_and_labels() {
        let usage = serde_json::json!({"input_tokens":2,"output_tokens":4,"cache_read_input_tokens":7,"cache_creation_input_tokens":1067,"cache_creation":{"ephemeral_1h_input_tokens":1067}});
        assert_eq!(
            UsageCounts::from_json(&usage),
            UsageCounts {
                input: 2,
                output: 4,
                cache_read: 7,
                cache_creation: 1067
            }
        );
        assert_eq!(
            UsageCounts::from_json(&serde_json::json!({})),
            UsageCounts::default()
        );
        let mut key = [0_u8; 32];
        key[0] = 0xab;
        key[5] = 0x01;
        assert_eq!(key_label(&key), "ab0000000001");
        assert_eq!(
            fresh_label(Some(&session::FreshReason::PrefixMismatch {
                expected: key,
                actual: key
            })),
            "prefix_mismatch"
        );
        assert_eq!(effect_label(&session::MapEffect::Keep), "keep");
    }
}

#[cfg(test)]
mod process_tests {
    use super::*;

    #[tokio::test]
    async fn registry_removes_reaped_groups_and_closes_spawning() {
        let groups = ProcessGroups::default();
        let spec = clod_core::spawn::SpawnSpec {
            program: "/usr/bin/true".into(),
            args: vec![],
            env_set: vec![],
            env_remove: vec![],
            working_directory: std::env::temp_dir().to_str().unwrap().into(),
        };
        let mut process = Process::spawn(spec.clone(), groups.clone()).unwrap();
        assert!(groups.0.lock().unwrap().live.contains(&process.group));
        Process::wait(&mut process.child, process.group, &process.groups)
            .await
            .unwrap();
        assert!(groups.0.lock().unwrap().live.is_empty());
        groups.shutdown();
        assert!(
            matches!(Process::spawn(spec, groups), Err(error) if error.kind() == io::ErrorKind::Interrupted)
        );
    }

    #[tokio::test]
    async fn reaped_child_ignores_later_group_kills() {
        let mut process = Process::spawn(
            clod_core::spawn::SpawnSpec {
                program: "/usr/bin/true".into(),
                args: vec![],
                env_set: vec![],
                env_remove: vec![],
                working_directory: std::env::temp_dir().to_str().unwrap().into(),
            },
            ProcessGroups::default(),
        )
        .unwrap();
        // Intercept signals rather than risk signalling an unrelated reused PGID.
        // This isolates the boundary even when a descendant holds a pipe open.
        process.group_signals = Some(0);
        process.child.wait().await.unwrap();
        assert!(process.child.id().is_none());
        process.kill(); // later timeout action while EOF is outstanding
        assert_eq!(
            process.group_signals,
            Some(0),
            "post-reap timeout must not signal a reusable group ID"
        );
    }
}
