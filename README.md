# clod

clod is a small Anthropic Messages API proxy that drives your locally authenticated Claude Code CLI using your Claude subscription. Polytoken supplies its system prompt and tools and executes all tool calls. clod forwards generation and resumes forked CLI sessions where history matches. It is intended for a single local user, not a multi-user gateway.

## Build and run

Install Rust and log into the `claude` CLI first, then:

```sh
cargo build --release --workspace
CLOD_API_KEY=test-key ./target/release/clod
curl -H 'x-api-key: test-key' http://127.0.0.1:8787/health
```

The configured work directory must be empty, including no `.git`. Claude CLI **2.1.319** is the tested version; startup logs a warning (not a refusal) if the version differs or cannot be checked. See [protocol evidence](docs/protocol.md) before upgrading.

### Configuration

All settings are environment variables. Defaults come from `crates/clod/src/lib.rs` and core `spawn::Config`.

| Variable | Default | Meaning |
|---|---|---|
| `CLOD_LISTEN` | `127.0.0.1:8787` | Listen address; keep local unless deliberately secured. |
| `CLOD_API_KEY` | unset | Optional inbound key via `x-api-key` or case-insensitive Bearer scheme; an empty configured key refuses startup. |
| `CLOD_MAX_BODY_BYTES` | `268435456` (256 MiB) | Maximum request body size, including image data. |
| `CLOD_ALLOW_ANY_HOST` | unset | Only `1` permits non-loopback Host headers; never permits Origin. |
| `CLOD_CLAUDE_BIN` | `claude` | CLI executable. |
| `CLOD_WORKDIR` | `$XDG_STATE_HOME/clod/workdir`, or `$HOME/.local/state/clod/workdir` | Empty dedicated CLI working directory, created if needed. |
| `CLOD_MAX_CONCURRENT` | `8` | Global concurrent exchanges; positive integer. |
| `CLOD_SESSION_CAP` | `256` | In-memory checkpoint LRU capacity. |
| `CLOD_PING_MS` | `15000` | SSE ping interval in milliseconds. |
| `CLOD_HOLD_IDLE_MS` | `60000` | Idle backstop for held tool hooks; firing invalidates checkpoint. |
| `CLOD_TURN_TIMEOUT_MS` | `900000` | Turn timeout in milliseconds. |
| `CLOD_MEASURE_LOG` | unset | Append decision/effect and numeric usage measurement records to this path, one complete JSON line per record (concurrent requests do not interleave). |
| `RUST_LOG` | `info` | Log filter for diagnostics on stderr, e.g. `debug` or `clod=debug`. |
| `CLOD_UPSTREAM_BASE_URL` | unset | Measurement-only CLI traffic redirection; must be HTTP on `127.0.0.1`, `localhost` or `[::1]` (optional port), otherwise startup fails. |
| `CLOD_SESSION_KEY` | unsupported | Any value is rejected: strict session keys are mandatory. |

Do not set `CLAUDE_CONFIG_DIR`; clod removes credential overrides from the child and preserves subscription authentication. Inherited `CLOD_*`, `ANTHROPIC_*`, `CLAUDE_CODE_USE_BEDROCK` and `CLAUDE_CODE_USE_VERTEX` are scrubbed; explicit core settings and the validated measurement override are then applied. Proxy variables (`HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY`, `NO_PROXY` and lowercase equivalents) are deliberately inherited: review them before running.

Requests carrying **any Origin header**, even `null` or localhost, get a 403 Anthropic error envelope. Host is required and must be `localhost`, `127.0.0.1` or `[::1]`, optionally with a numeric port, unless `CLOD_ALLOW_ANY_HOST=1`. This is a local-service protection, not a CORS endpoint. The working directory must also be outside every git repository, including parent directories with a `.git` file or folder.

Endpoints: `POST /v1/messages`, `GET /health`, and a static `GET /v1/models` stub advertising `claude-sonnet-4-6`. The model list is **not** CLI capability discovery; requests may select other CLI-supported models. Other paths return 404. Response bodies finish once the core emits the terminal response, while process cleanup and the per-conversation lock remain active until Done.

### Logging

Diagnostics go to stderr at `info` by default (colour only on a terminal, and never with a non-empty `NO_COLOR`). Stdout carries a single `listening address=<host:port>` line once the listener is bound, for supervisors and scripts that discover the port.

Each request's records are in a `request{id=N}` span; `N` matches the `request` field of `CLOD_MEASURE_LOG` records. At `info` a request logs `request received` (model, stream flag, message and tool counts), `session planned` (a 12-hex-digit prefix of the session key and why a fresh session would be needed, if it would), `spawning CLI` and `CLI exited` for each attempt (fresh or resume, and the resumed CLI session ID), then `request finished` with HTTP status, attempts, checkpoint effect and reason, elapsed time and the token counts from the final attempt's CLI `result` record: `input_tokens`, `output_tokens`, `cache_read_input_tokens` and `cache_creation_input_tokens`. These counts are omitted when the attempt produced no `result`. Streaming usage records are not summed. Prompt, tool and account contents are not logged; CLI stderr lines are logged at `debug` only.

### Shutdown

SIGTERM, SIGINT and SIGHUP stop accepting connections and exit with status 143, 130 and 129, respectively. These statuses follow the `128 + signal` convention; clod returns an exit code rather than re-raising the signal. SIGQUIT retains its default disposition, including core-dump semantics.

Shutdown does **not** drain requests: in-flight responses may be cut off. Before waiting for runtime workers, clod prevents new CLI spawns and SIGKILLs each unreaped CLI's process group, including descendants that ignore stdin EOF. Spawn, reap and group cleanup are serialized so cleanup never signals a previously reaped CLI's group, whose ID could have been reused. Runtime teardown then waits at most three seconds for workers; a worker blocked in synchronous measurement or logging I/O cannot indefinitely delay exit or skip group cleanup. Further SIGTERM/SIGINT/SIGHUP signals during this bounded wait remain handled and do not bypass cleanup or extend the deadline.

### Point Polytoken at clod

Adapt the provider/model configuration used by the [Polytoken harness](docs/polytoken-harness.md):

```toml
defaults.full = "m1"
[providers.gateway]
kind = { type = "custom_anthropic_compatible" }
url = "http://127.0.0.1:8787"
auth = { type = "static_key", key = "test-key", format = "anthropic_x_api_key" }
[models.m1]
provider_name = "claude-sonnet-4-6"
variant = "claude"
provider = "gateway"
class = "full"
context_window = 200000
supports_vision = true
```

Match the key to `CLOD_API_KEY`, and select a CLI-supported model.

## What the model sees

The model sees Polytoken's system prompt and client tools exposed as `mcp__oc__<name>`, not Claude Code's normal built-in tools. Tool calls are forwarded as the model emitted them, minus the `mcp__oc__` prefix, even when the name was not declared (as the Anthropic API does); the CLI never executes them, and Polytoken answers unknown tools with an error result the model can recover from. Fresh replay adds a fixed provenance note. This is **not an injection-free replacement transport**: accepted Phase 0 captures also show a CLI billing metadata line, Agent SDK identity, a `# Environment` system message (working directory/platform/date/model and instructions), and an account email reminder in the first user message. See [the exact capture and limitations](docs/protocol.md).

## Resume and caching

### Checkpoints and resume

clod keeps a bounded, in-memory map of CLI checkpoints. Each key combines the conversation identity (`X-Polytoken-Conversation`, falling back to `X-Polytoken-Session`), model, system prompt and tool definitions. A checkpoint records the CLI session, an assistant-message boundary, a hash of the conversation prefix and any pending tool-call IDs.

On the next request, clod checks that the echoed history matches the saved prefix and that every pending tool call has exactly one result, with no duplicates or unknown IDs. It then forks the CLI session at the saved assistant boundary and supplies only the new user messages. Consecutive trailing user messages are merged into one input while preserving block order. Forking before the CLI's tool-denial records lets the client supply the real tool results instead.

A checkpoint is saved only when the CLI records provide a safe resume boundary. Streamed and recorded tool IDs must agree, and CLI-generated results must not be interleaved before later tool calls. Mixed bare and prefixed tool names, additional assistant messages, or CLI media removal also prevent checkpointing. These checks avoid resuming history that differs from what the client saw.

### Fresh replay

Without a matching checkpoint, clod starts a new CLI session and renders the supplied conversation into a replay message. This happens after a restart or map eviction, when the model/system/tools change, when history is edited, or when a turn cannot safely be checkpointed. A stale resume can retry fresh once before the response starts.

Replay represents earlier roles, calls and results as text, while retaining image and document blocks. It includes a provenance note and neutralizes closing `</conversation_history>` tags inside history, but it is not a structurally isolated or injection-proof representation. Optional tool-name alignment changes only the CLI-facing presentation; the client's canonical history remains unchanged.

### Provider caching

CLI resume and provider prompt caching are separate mechanisms. Resume preserves native conversation structure and usually allows the provider to reuse most of the preceding input. Fresh replay changes that structure and can reduce cache reuse, although shared system/tool prefixes may still hit the provider cache. Changes to the system prompt, exposed tools or history can invalidate cached prefixes.

Checkpoints are neither persisted nor shared between clod processes. Provider caches can outlive a clod restart, but clod cannot guarantee their lifetime or reuse. Logs report cache-read and cache-creation token counts; a cache hit alone does not prove that a CLI session resumed. See [cache measurements](docs/cache-measurements.md) and [protocol details](docs/protocol.md) for evidence and verification procedures.

### Transcript storage

Forked transcripts accumulate under `~/.claude/projects/<encoded-workdir>/` independently of the in-memory checkpoint cap. There is no automatic transcript garbage collection. To clean up, stop clod and its CLI children, identify the folder for your dedicated `CLOD_WORKDIR`, and remove only the unwanted transcripts or that folder. Do not delete unrelated projects or credential/settings files. Removing a transcript makes its checkpoint unavailable and may require fresh replay.

## License

Licensed under the [Apache License, Version 2.0](LICENSE).

## Caveat

This uses Claude Code subscription authentication, not an Anthropic API key. Subscription access, rate limits, billing and permitted third-party use depend on Anthropic's current terms and your plan. Verify that your use is permitted; clod does not bypass limits or provide a compatibility guarantee.
