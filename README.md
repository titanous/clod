# clod

clod is a small Anthropic Messages API proxy that drives your locally authenticated Claude Code CLI using your Claude subscription. Polytoken supplies its system prompt and tools and executes client tool calls. clod forwards generation and resumes forked CLI sessions where history matches. It is intended for a single local user, not a multi-user gateway.

## Build and run

Install Rust and log into the `claude` CLI first, then:

```sh
cargo build --release --workspace
CLOD_LISTEN=127.0.0.1:3456 CLOD_API_KEY=test-key ./target/release/clod
# In another terminal:
curl -H 'x-api-key: test-key' http://127.0.0.1:3456/health
```

Port **3456** is a custom `CLOD_LISTEN` value used by these examples; the default is **8787**. `test-key` is a placeholder; use the same key in clod and your client.

`CLOD_WORKDIR` is created if needed, but must be empty **at every startup** and outside any git repository: neither it nor any parent directory may contain a `.git` file or folder. Claude CLI **2.1.319** is the tested version; startup logs a warning (not a refusal) if the version differs or cannot be checked. See [protocol evidence](docs/protocol.md) before upgrading.

### Configuration

All settings are environment variables. Defaults come from `crates/clod/src/lib.rs` and core `spawn::Config`.

| Variable | Default | Meaning |
|---|---|---|
| `CLOD_LISTEN` | `127.0.0.1:8787` | Listen address; keep local unless deliberately secured. |
| `CLOD_API_KEY` | unset | Optional inbound key via `x-api-key` or case-insensitive Bearer scheme; an empty configured key refuses startup. |
| `CLOD_MAX_BODY_BYTES` | `268435456` (256 MiB) | Maximum request body size, including image data. |
| `CLOD_ALLOW_ANY_HOST` | unset | Only `1` permits non-loopback Host headers; never permits Origin. |
| `CLOD_CLAUDE_BIN` | `claude` | CLI executable. |
| `CLOD_WORKDIR` | `$XDG_STATE_HOME/clod/workdir`, or `$HOME/.local/state/clod/workdir` | Dedicated CLI working directory; startup requirements above apply. |
| `CLOD_MAX_CONCURRENT` | `8` | Global concurrent exchanges; positive integer. |
| `CLOD_SESSION_CAP` | `256` | In-memory checkpoint LRU capacity. |
| `CLOD_PING_MS` | `15000` | SSE ping interval in milliseconds. |
| `CLOD_HOLD_IDLE_MS` | `60000` | Idle backstop for held tool hooks; firing invalidates checkpoint. |
| `CLOD_TURN_TIMEOUT_MS` | `900000` | Turn timeout in milliseconds. |
| `CLOD_ALIGN_TOOL_NAMES` | unset (disabled) | Experimental CLI-facing tool-name alignment; only `1` is accepted when set. See [alignment details](docs/instruction-name-alignment.md). |
| `CLOD_WEEKLY_BREAKER` | unset (disabled) | Only `1` enables the rolling-hour weekly-usage guard; every other set value fails startup. See [subscription usage](docs/subscription-usage.md). |
| `CLOD_WEEKLY_BREAKER_PERCENT` | `1` | Weekly utilization rise threshold in percentage points within a rolling hour; rejects only a strictly greater rise. Must be finite, greater than 0 and at most 100, even with the guard disabled. Does not enable the guard by itself. |
| `CLOD_MEASURE_LOG` | unset | Append decision/effect, token usage, subscription quota and rate-limit diagnostics as JSONL; create the parent directory first. |
| `RUST_LOG` | `info` | Log filter for diagnostics on stderr, e.g. `debug` or `clod=debug`. |
| `CLOD_UPSTREAM_BASE_URL` | unset | Measurement-only CLI traffic redirection; must be HTTP on `127.0.0.1`, `localhost` or `[::1]` (optional port), otherwise startup fails. |

Do not set `CLAUDE_CONFIG_DIR`; clod removes credential overrides from the child and preserves subscription authentication. Inherited `CLOD_*`, `ANTHROPIC_*`, `CLAUDE_CODE_USE_BEDROCK` and `CLAUDE_CODE_USE_VERTEX` are scrubbed; explicit core settings and the validated measurement override are then applied. Proxy variables (`HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY`, `NO_PROXY` and lowercase equivalents) are deliberately inherited: review them before running.

Requests carrying **any Origin header**, even `null` or localhost, get a 403 Anthropic error envelope. Host is required and must be `localhost`, `127.0.0.1` or `[::1]`, optionally with a numeric port, unless `CLOD_ALLOW_ANY_HOST=1`. This is a local-service protection, not a CORS endpoint.

Endpoints: `POST /v1/messages`, `GET /health`, and `GET /v1/models`. The model list is a static placeholder advertising `claude-sonnet-4-6`, not capability discovery or an availability restriction; requests can select other CLI-supported models, such as the example below. Other paths return 404. Response bodies finish at the terminal response; CLI cleanup can continue afterward before the per-conversation lock is released.

### Logging

Diagnostics go to stderr (`info` by default, controlled by `RUST_LOG`); stdout prints one `listening address=<host:port>` line when bound. Request logs cover session planning, CLI attempts, completion, elapsed time and final-attempt token counts when available. Request IDs correlate with measurement records. CLI stderr is logged only at `debug`.

For a durable measurement log, create its parent directory and set the path when starting clod:

```sh
mkdir -p "$HOME/.local/state/clod/logs"
CLOD_LISTEN=127.0.0.1:3456 CLOD_API_KEY=test-key \
  CLOD_MEASURE_LOG="$HOME/.local/state/clod/logs/measurements.jsonl" \
  ./target/release/clod
```

Token usage and cache counts are not subscription quota percentages. See [token/cache measurements](docs/cache-measurements.md), [subscription usage observations and the optional guard](docs/subscription-usage.md), and [429 sources and timing diagnostics](docs/protocol.md#rate-limit-diagnostics). Streaming usage records are not summed.

### Shutdown

SIGTERM, SIGINT and SIGHUP stop accepting connections and exit with status 143, 130 and 129, respectively (`128 + signal`). SIGQUIT retains its default disposition.

Shutdown does **not** drain requests: in-flight responses may be cut off. clod prevents new CLI spawns, kills unreaped CLI process groups (including descendants), and allows at most three seconds for runtime workers to finish. Further handled signals do not extend that deadline.

### Point Polytoken at clod

Merge this illustrative provider/model example into your [Polytoken configuration](https://docs.polytoken.dev/reference/configuration/):

```yaml
providers:
  claude:
    kind: {type: custom_anthropic_compatible}
    url: http://127.0.0.1:3456
    auth: {type: static_key, key: test-key, format: bearer_token}
    protocol: anthropic_messages
models:
  'claude/claude-opus-5-5':
    provider_name: claude-opus-5-5
    variant: claude
    class: full
    max_output_tokens_per_turn: 65536
    provider: claude
    reasoning:
      type: effort
      effort_set: custom
      can_disable: true
      levels: [low, medium, high, xhigh, max]
      default_level: medium
    context_window: 1000000
    compaction_threshold: 0.88
    supports_vision: true

  'claude/claude-fable-5-1':
    provider_name: claude-fable-5-1
    provider: claude
    variant: claude
    class: full
    max_output_tokens_per_turn: 65536
    reasoning:
      type: effort
      effort_set: custom
      can_disable: true
      levels: [low, medium, high, xhigh, max]
      default_level: high
    edit_format: search_replace
    context_window: 1000000
    compaction_threshold: 0.8
    supports_vision: true

  'claude/claude-sonnet-5-5':
    provider_name: claude-sonnet-5-5
    provider: claude
    variant: claude
    class: full
    reasoning:
      type: effort
      effort_set: custom
      can_disable: true
      levels: [low, medium, high, xhigh, max]
      default_level: medium
    context_window: 1000000
    compaction_threshold: 0.8
    supports_vision: true

  'claude/claude-haiku-5-5':
    provider_name: claude-haiku-5-5
    provider: claude
    variant: claude
    class: mini
    reasoning:
      type: effort
      effort_set: custom
      can_disable: true
      levels: [low, medium, high, xhigh, max]
      default_level: high
    context_window: 1000000
    compaction_threshold: 0.8
    supports_vision: true
```

Match the URL to `CLOD_LISTEN` and the key to `CLOD_API_KEY`, then select one of these model entries in Polytoken. The model name, limits and reasoning levels are illustrative settings, not a guarantee of support for every model, CLI version or subscription. See the [Polytoken harness](docs/polytoken-harness.md) for isolated testing.

## What the model sees

The model sees Polytoken's system prompt and client tools exposed as `mcp__oc__<name>`, rather than Claude Code's normal built-in tool set. clod forwards client tool calls with that prefix removed, including undeclared names; Polytoken executes them or returns an unknown-tool error. CLI tool-denial records can still appear in transcripts. Fresh replay adds a fixed provenance note.

This is **not an injection-free replacement transport**: the CLI/provider path can add billing metadata, Agent SDK identity, environment instructions and account reminders. See [the captured prompt, protocol and limitations](docs/protocol.md) for details.

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

Forked transcripts accumulate under `~/.claude/projects/<encoded-workdir>/` independently of the in-memory checkpoint cap. There is no automatic transcript garbage collection. To clean up, stop clod and its CLI children, identify the folder for your dedicated `CLOD_WORKDIR`, and remove only the unwanted transcripts or that folder. Removing a transcript makes its checkpoint unavailable and may require fresh replay.

## License

Licensed under the [Apache License, Version 2.0](LICENSE).

## Caveat

This uses Claude Code subscription authentication. Subscription access, rate limits, billing and permitted third-party use depend on Anthropic's current terms and your plan. Verify that your use is permitted; clod does not bypass limits or provide a compatibility guarantee.
