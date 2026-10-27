# clod

clod is a small Anthropic Messages API proxy that drives your locally authenticated Claude Code CLI using your Claude subscription. Polytoken supplies its system prompt and tools and executes all tool calls. clod forwards generation and resumes forked CLI sessions where history matches. It is intended for a single local user, not a multi-user gateway.

## Build and run

Install Rust and log into the `claude` CLI first, then:

```sh
cargo build --release --workspace
CLOD_API_KEY=test-key ./target/release/clod
curl -H 'x-api-key: test-key' http://127.0.0.1:8787/health
```

The configured work directory must be empty, including no `.git`. Claude CLI **2.1.308** is the tested version; startup logs a warning (not a refusal) if the version differs or cannot be checked. See [protocol evidence](docs/protocol.md) before upgrading.

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

Checkpoint keys include conversation identity (`X-Polytoken-Conversation`, falling back to `X-Polytoken-Session`), model, system and tools. The echoed history must match the checkpoint hash; consecutive trailing user messages are merged into one CLI input with blocks in order. Pending calls require exactly one result each, without duplicates or unknown IDs. Resume forks at the last forwarded-tool assistant UUID (or last text assistant UUID), dropping CLI hook denials. Declared bare tool calls are forwarded unchanged. Turns whose forwarded calls are all bare names exactly matching declared tools may checkpoint under the same safety rules as prefixed turns, based on the clean 2026-10-26 live fork evidence. Mixed bare/prefixed turns still discard with `MixedBareAndPrefixedToolCalls`: that case could not be proved live. Tool checkpoints must be the final assistant record, and streamed/recorded tool IDs must agree exactly; repeated CLI messages also force replay.

Missing identity/checkpoints, changed system/tools/model, edited history, invalid results, stale CLI sessions, map eviction and server restart cause fresh replay. A stale resume can retry fresh once before any response is committed. Replay rewrites history and can lose native history cache reuse. Closing `</conversation_history>` tags inside history are neutralized; replay framing is otherwise plain text, and replayed roles are not structurally protected.

Cache measurements found stable requests recover caching; system changes, deferred-tool exposure and facet switches invalidate prefixes. A warm shared static prefix does not prove native history reuse. On CLI 2.1.308, image-turn merging/resume and signed thinking (streaming, non-streaming, adaptive tool continuation) were verified live on 2026-10-26. A tool-result image also resumed successfully. Bare-name forwarding and a clean bare-call fork were observed live; bare-only checkpoint eligibility is regression-tested offline, while mixed turns remain disabled. See [protocol evidence](docs/protocol.md#live-verification-2026-10-26) and [cache findings](docs/cache-measurements.md).

Only with explicit operator approval, run the ignored direct-HTTP checks with:

```sh
OPERATOR_APPROVED_LIVE_VERIFY=1 CLOD_LIVE=1 cargo test -p clod --test live_verify -- --ignored --nocapture --test-threads=1
```

Each test makes three expected upstream requests (image: Haiku; thinking: Sonnet), with a hard four-attempt ceiling including CLI retries. Raw evidence and dedicated HOME/workdir/transcript folders are retained for operator cleanup. Thinking generation is model-dependent; a missing thinking block fails rather than claiming pass-through was verified.

Forked transcripts accumulate under `~/.claude/projects/<encoded-workdir>/`, even though the in-memory checkpoint map is capped. There is no automatic transcript garbage collection. Stop clod and any child CLI first, identify only the folder for your dedicated `CLOD_WORKDIR`, and manually remove unwanted transcripts or that dedicated folder. Do not delete unrelated projects or credential/settings files; removing transcripts causes replay on the next use.

## Caveat

This uses Claude Code subscription authentication, not an Anthropic API key. Subscription access, rate limits, billing and permitted third-party use depend on Anthropic's current terms and your plan. Verify that your use is permitted; clod does not bypass limits or provide a compatibility guarantee.
