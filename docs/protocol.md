# Claude CLI protocol capture — Phase 0

## Contradictions — reviewed and accepted by the operator on 2026-10-26; Phase 2 proceeds

The injected email reminder, environment block, and billing/SDK identity lines are accepted known differences, as in Meridian today. Mitigations: use a fixed, empty, non-git working directory supplied by configuration and forward the client's `output_config.effort` with `--effort`.

CLI **2.1.308 (Claude Code)** injects content beyond a short identity line even with a plain-string `systemPrompt`, `tools: []`, strict MCP config, empty setting sources/plugins, and `CLAUDE_CODE_DISABLE_ATTACHMENTS=1`:

1. An automatic `<system-reminder>` in the first user message contains `# userEmail`, the account email, instructions about its use, and a statement that Claude Code attached the context automatically.
2. A separate **system-role message in `messages`** contains `# Environment`, the working directory, git status, platform, shell, OS version, a substantial untrusted-download instruction, model identity/knowledge cutoff, and today's date. It also carries `output_config.effort: medium`.
3. The `system` array includes a billing metadata line as well as the Agent SDK identity and the supplied system prompt.

Evidence: [`upstream.ndjson`](../crates/clod-core/tests/fixtures/upstream.ndjson), bodies of `/v1/messages?beta=true`. Email and home paths are sanitized. The full exact text, except those redactions, is in that fixture. **No workaround or redesign was attempted. Live model calls stopped once this was observed.**

## Scope, version, and cost accounting

Captured with `/Users/<user>/.local/bin/claude --version`: `2.1.308 (Claude Code)`, vendored Meridian Agent SDK, Node at `/opt/homebrew/bin/node`. Meridian was read-only and was neither built nor modified. No config or credentials were modified and `CLAUDE_CONFIG_DIR` was never set. CLI-created conversation transcripts are normal effects of these consented calls.

**14 live CLI query invocations:** 12 generated turns and 2 stale refusals before generation. All used `haiku` except one `sonnet` thinking-option probe. The CLI may itself make additional model requests: the two upstream-routed invocations recorded **three** `/v1/messages?beta=true` requests, plus two `/api/hello` requests. This is not a claim that CLI invocations equal billed requests. No limits, auth failures, or context overflow were deliberately triggered.

The operator accepted the known injection differences on 2026-10-26 and authorized Phase 2. Fixtures (a)–(f) exist; lifecycle ordering, live interrupt/thinking blocks, and real Polytoken byte stability remain unverified below. The spawn spec defaults enabled thinking without a display to `--thinking-display summarized`, following Meridian's `passthroughThinking`/`sdkThinking`; explicit unknown displays are dropped. `thinking-meridian-synthetic.ndjson` is derived from Meridian's thinking tests and generic signature-delta handling, not a live capture. Thinking/signature/redacted-thinking events are forwarded unchanged.

## Capture files and limitations

- `scripts/capture-claude.sh`: NUL-delimited argv, raw stdin/stdout/stderr through tee, then `exec` of the installed binary; logs no environment or headers.
- `scripts/capture.mjs`: SDK driver, tools `lookup(key)` and `combine(values)`, deny-hold hook, one-turn cap, empty dedicated `scripts/captures/workdir`, text/tool/parallel/thinking/image/stale scenarios.
- `scripts/capture-upstream.mjs`: loopback body-only HTTPS forwarder to `api.anthropic.com`; never logs request or response headers. Subscription auth through the custom base URL **worked**. Its background shell job later reported exit 1 because the harness timed it out after 10 seconds; captured requests succeeded before that. A clean proxy shutdown was not verified.
- `scripts/sanitize-captures.mjs`: builds sanitized fixtures and `capture-evidence.json` from existing raw captures, plus explicitly synthetic error fixtures.
- Raw local captures: `scripts/captures/phase0/<scenario>/`, upstream body log `scripts/captures/upstream.ndjson`. Raw CLI initialization exposes account metadata; do not publish it.
- Fixtures are one JSON object per line, `{dir: in|out|err|upstream, t_ms, line}`. Synthetic fixtures add `synthetic:true` and source attribution. Pipe fixture `t_ms:0` means **timing unavailable**, not simultaneous events. Each pipe's record order is retained, but stdin records are grouped before stdout before stderr. Do not infer cross-pipe ordering from these fixtures. `capture-evidence.json` separately retains real SDK event times and argv.
- Hashes were computed before/after every fork. Separate scenario commands overwrote the aggregate `summary.json`; consequently only the upstream pair's exact hashes survive there. Earlier equality checks survive in tool execution output, not a durable hash artifact. The driver now additionally saves per-turn `summary.json` to prevent this on recapture.

## Local help check during offline safety review

`claude --help` was run locally without a model call. The public help contained no `--thinking` option (a filtered search returned no matches), so this review did **not** change the captured `--max-thinking-tokens` mapping or invent adaptive/budget syntax. Adaptive mapping remains pending explicit local help evidence for a supported replacement. The Phase 0 enabled-budget capture below remains the available flag evidence.

## SDK options → exact argv / initialization

Common logged argv (the SDK did **not** emit `-p`):

```text
--output-format stream-json --verbose --input-format stream-json
--max-turns 1 --model haiku
--allowedTools mcp__oc__lookup,mcp__oc__combine
--tools --setting-sources= --strict-mcp-config
--permission-mode bypassPermissions --allow-dangerously-skip-permissions
--include-partial-messages
```

The empty `tools` list produces a standalone `--tools` with **no empty-string argument**. Empty setting sources is exactly `--setting-sources=`. `plugins: []` produces no plugin flag. The plain-string system prompt is sent in the **initialize control request**, not argv; the SDK serializes it as a one-element string array. `mcpServers` and hooks likewise travel through initialize. `thinking:{type:enabled,budgetTokens:1024}` adds `--max-thinking-tokens 1024` (probe model `sonnet`). No thinking-display flag was observed.

Resume adds:

```text
--resume <source-session-id> --fork-session --resume-session-at <assistant-uuid>
```

Environment set: `CLAUDE_CODE_DISABLE_ATTACHMENTS=1`, `ENABLE_CLAUDEAI_MCP_SERVERS=false`, `ENABLE_TOOL_SEARCH=false`, `CLAUDE_CODE_MAX_OUTPUT_TOKENS=2048`. Driver removes inherited `CLAUDE_CONFIG_DIR`, API-key/auth-token overrides and `CLAUDECODE`, preserving subscription login. This host was not root, so no `IS_SANDBOX` override was needed. The SDK may add its own environment entries; environment was intentionally not logged.

## Wire shapes

Examples below abbreviate IDs; exact live records are in `b-tool.ndjson`.

### Initialize (stdin)

```json
{"request_id":"id","type":"control_request","request":{"subtype":"initialize","hooks":{"PreToolUse":[{"matcher":"mcp__oc__.*","hookCallbackIds":["hook_0"]}]},"sdkMcpServers":["oc"],"systemPrompt":["You are a precise test assistant. Follow the user instructions exactly. Use only the provided tools when requested."]}}
```

CLI responds with `type:control_response`, `response:{subtype:success,request_id,response:{...}}`. The initialization payload includes model capabilities, CLI version, PID, account metadata, permission mode and `hooks_applied:true`; unknown fields should not be assumed stable.

### MCP (stdout request, stdin reply)

```json
{"type":"control_request","request_id":"id","request":{"subtype":"mcp_message","server_name":"oc","message":{"jsonrpc":"2.0","method":"tools/list","id":1}}}
{"type":"control_response","response":{"subtype":"success","request_id":"id","response":{"mcp_response":{"jsonrpc":"2.0","id":1,"result":{"tools":[]}}}}}
```

Sequence observed: MCP `initialize` (protocolVersion `2025-11-25`, JSON-RPC id 0), `notifications/initialized` (no JSON-RPC id), `tools/list` (id 1). The SDK responds even to the notification with an empty result and id 0. Tool list uses `inputSchema`, not `input_schema`; SDK adds JSON Schema draft-07 and `execution:{taskSupport:forbidden}`. No `tools/call` arrived: the hook prevented execution.

### Hook

```json
{"type":"control_request","request_id":"id","request":{"subtype":"hook_callback","callback_id":"hook_0","input":{"hook_event_name":"PreToolUse","tool_name":"mcp__oc__lookup","tool_input":{"key":"alpha"},"tool_use_id":"toolu_id","session_id":"session","transcript_path":"<HOME>/...","cwd":"<HOME>/...","mcp_server":{"name":"oc","source":"sdk"}},"tool_use_id":"toolu_id"}}
{"type":"control_response","response":{"subtype":"success","request_id":"id","response":{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"Forwarded to client. End your turn."}}}}
```

Hook input also contains prompt id, permission mode and effort. Hooks **do fire for SDK MCP tools**. No test tool implementation ran.

### Results and streaming

`stream_event` contains `event` (Anthropic `message_start`, `content_block_*`, `message_delta`, `message_stop`), plus CLI session/UUID fields. Assistant records have `type:assistant`, `uuid`, `session_id`, and `message.content`. Denied calls produce user tool-result records, then a terminal `result`.

Text terminal: `type:result, subtype:success, is_error:false, num_turns:1`, result text, usage, modelUsage, session_id, uuid and duration fields. Tool terminal: `subtype:error_max_turns, is_error:true, errors:["Reached maximum number of turns (1)"]`. The SDK yields this result **then throws** `Claude Code returned an error result: Reached maximum number of turns (1)`; this SDK exception does not mean the CLI omitted result.

`interrupt`: **not captured live** before the contradiction gate stopped calls. Implemented from vendored `sdk.d.ts` `SDKControlInterruptRequest`: `{"type":"control_request","request_id":"clod-N","request":{"subtype":"interrupt"}}`. This declaration-derived shape remains unverified live. EOF/exit behavior was not independently instrumented; completed SDK iteration and `q.close()` do not prove that stdin EOF alone causes exit after result.

## Live verification: 2026-10-26

Claude CLI **2.1.308**, clod **0.1.0**; direct HTTP, no Polytoken process. The operator explicitly approved investigating the three outstanding behaviors. **25 upstream `/v1/messages` requests: Haiku 15 / Sonnet 10**, actual models `claude-haiku-5-5` and `claude-sonnet-5-5`; zero captured API errors, no ceiling hits. Raw local evidence is `scripts/captures/live-verify-20261026-{182054,182301,182353,182439,182611,182619,182706}/`. These are separate from the older measurement ledger. Only dedicated transcripts were read; credentials/config/Keychain were not changed, nothing was deleted, and real HOME was restored only for the Claude child. The driver retains raw CLI stdin/stdout, argv, HTTP replies, measurement decisions, and Phase 0 recorder bodies. Bodies are parsed JSON, not original wire slices.

### Bare-name calls: single-call fork clean; conservative discard retained

Earlier session `8a6656fd-629e-49b3-a5b4-9e1838fabbb4`: record 17 assistant text → record 18 assistant `c91e8d3e-8691-403c-8d40-32d0faa31efe` with bare `tool_search` → record 19 user `a09abfed-f2f1-47f6-aebb-7e358b482ef5`, parented to record 18, with `<tool_use_error>Error: No such tool available: tool_search</tool_use_error>`. The error is a **descendant**, not an ancestor, of the candidate tool checkpoint. Attachment records 20–21 descend from the error.

New run 182054 upstream body 4 provoked bare `lookup` on Haiku. Session `c7987a7f-87d2-42d9-922c-cba4b9d45eb2` record 15 assistant `e14cdb3d-9e7d-4691-a75d-de11233106ac` contains tool ID `toolu_01Qva23KkpYqaUuuEhBRHNgd`; record 16 is its child user error `No such tool available: lookup`. Run 182301 used a measurement wrapper to add `--resume <session> --fork-session --resume-session-at <record-15-uuid>` and supply the real result via direct HTTP/clod initialization. Upstream body 1 contains exactly:

```text
assistant tool_use(name=lookup, id=toolu_01Qva23KkpYqaUuuEhBRHNgd)
user tool_result(tool_use_id=toolu_01Qva23KkpYqaUuuEhBRHNgd, content=REAL_BARE_VALUE)
```

No duplicate/orphan result or saved error placeholder; reply reports `REAL_BARE_VALUE`, no API error. This proves the **single bare call** fork boundary is clean. A second bare phrasing refused to call; the mixed bare/prefixed prompt in run 182439 produced **two prefixed calls**, not the desired mixed case. Therefore mixed-turn checkpoint safety was not proved. At the end of the live investigation all bare turns still discarded. In the subsequent operator-approved **offline follow-up**, the core enables normal checkpoint eligibility only for turns whose forwarded calls are all bare names exactly matching declared tools; mixed bare/prefixed turns discard with `MixedBareAndPrefixedToolCalls`. A comment records the unproved mixed live case. The final assistant must carry a forwarded call, streamed and recorded IDs must match, and the existing trailing-content, message-count and other safety gates remain intact. Sanitized `bare-lookup.ndjson` and existing `native-search.ndjson` both regression-test Commit, pending IDs and echoed real-result Resume; native-search violates none of those gates. No new live calls were made for this policy change. There is still no reliably provokable ignored bare-name live test. The probe mode contains historical session IDs and is evidence tooling, not a portable test.

### Bare-name batches: earlier calls' errors are ancestors of the resume point

Observed in the running service's own CLI transcripts (CLI 2.1.308, no extra live calls), not in a dedicated capture. In session `e6363ff7-0a51-45fc-b762-456de703242c`, one upstream message `msg_011CgSrs281fMvMNETXtEUdZ` produced bare `shell_exec` then bare `file_read`. The CLI split it into: record 29 assistant `346770bc` (`shell_exec`) → record 30 user `6b7a1aef`, `No such tool available: shell_exec`, written 1 ms later while the message was still streaming → record 31 assistant `d69f4487` (`file_read`, same message id) **parented to the error** → record 32 user error for `file_read`. clod checkpointed at `d69f4487`. In the resumed session `ddccfa9b-2f04-4b1c-b11d-8742e56c3de9`, the client's real results for both IDs were appended under `d69f4487`, but the CLI's upstream request carried the `shell_exec` **error** and only the `file_read` real result. The client had executed both calls. The model therefore saw "No such tool available" for every call except the last, even though the calls had run. Prefixed parallel calls do not have this shape: held hooks place every CLI result after the last tool record (`c-parallel`).

The core now records CLI `user` tool_result records. If a forwarded tool record follows one in the same turn, the turn discards with `ToolResultBeforeLastToolCall` and the next turn replays client history. Single bare calls, and CLI results after the last tool record, keep normal eligibility. Results are not filtered or rewritten. Regression tests: `crates/clod-core/tests/tool_batches.rs`. This fix has offline regression coverage; no dedicated live verification of the changed checkpoint policy has been run. Affected turns replay rather than resume, which can reduce cache reuse.

Separately, the inspected calls to `progress_update` and `exit_tool` already carry JSON-looking strings rather than arrays in the CLI transcript. The raw `input_json_delta` in `fixtures/live/native-search.ndjson` likewise shows a bare call's integer arriving as `"3"`. clod forwards these values unchanged. The observed array failures are upstream of clod's forwarding; the saved transcript alone does not distinguish model generation from CLI serialization. The forwarding regressions check both correctly typed arrays and string-encoded arrays without attempting to repair inputs.

### Images: merged turn and next turn resume

Runs 182054 and passing ignored `live_image_resume` run 182706 have spawn plans **Fresh, Resume, Resume**, one upstream request each. Body 2 ends in one user message with blocks **64×64 red/blue PNG → client prompt → CLI image-source text**. PNG base64 is byte-equal to client input; the CLI adds a trailing newline to client text and `[Image: source: /private/tmp/claude-501/…]`. Body 3 retains the same image bytes. Replies identify red then blue and retain `image-cobalt`. Run 182439 bodies 2–3 additionally verified a prefixed tool call followed by an image nested inside its real `tool_result`; Resume succeeded and answered `Red, then blue.` No generalization to rejected 1×1 images is intended.

The first ignored image run (182611) generated successfully but its assertion incorrectly expected the prompt to be the last upstream block. The corrected assertion accounts for the recorded CLI newline/source text; rerun 182706 passed. All six requests are included in the ledger.

### Thinking/signatures: streaming, non-streaming and tool resume

Local `claude --version` returned 2.1.308; public `--help` exposes effort but no thinking flag. Existing clod mapping remains: enabled budget → `--max-thinking-tokens <budget>`, adaptive without budget → no budget flag, either defaults display to `--thinking-display summarized`. Captured upstream bodies for enabled and adaptive requests both contain `thinking:{type:adaptive,display:summarized}` on Sonnet 5.5. Thus the CLI/model selects adaptive; do not claim the numeric budget is enforced upstream. No mapping change was needed.

Run 182054 streaming body 6 returned 344 thinking characters and a 1392-character signature. Bodies 7–9 preserve that block/signature exactly while all three follow-ups plan Resume. Run 182353 non-stream returned 669 thinking characters/signature 1704; its next adaptive stream emitted thinking before tool_use, then the real-result follow-up resumed successfully.

Passing ignored `live_thinking_resume`, run 182619: non-stream thinking 431/signature 1712; adaptive streamed thinking 380/signature 1716 before prefixed lookup. **All 45 thinking/signature deltas compare exactly CLI → client.** Source session `bcd281b6-2794-4eee-8f71-9216dfea513c`: records 19 thinking → 20 tool assistant `43bb7166-ebb3-4365-959c-09ab7f97d8e1` → 21 denial. Fork `e57e54c3-db2d-4f17-b56c-f04e61110731` has records 15 thinking → 16 tool → 17 real result/text → 23 answer. Upstream body 3 preserves both signed thinking blocks and `DEEP_REAL_VALUE`, without the denial or invalid-signature error. Spawn plans are Fresh, Resume, Resume. Client echo of signed thinking therefore does not invalidate history matching.

Ignored tests are in `crates/clod/tests/live_verify.rs`, driven by `scripts/live-verify.py`; both passed live. Each requires `CLOD_LIVE=1` plus `OPERATOR_APPROVED_LIVE_VERIFY=1`, expects three requests, and records at most four attempts including retries. That live investigation required no production core changes or new dependencies; the subsequent offline bare-only policy change is described above.

### Offline follow-up verification

No additional live calls or commits. Fake-CLI logs now publish a mutex-protected complete NDJSON snapshot by same-directory temporary write + atomic rename, preventing every shell log reader from seeing partial records. A unit regression keeps an old reader open while publishing a larger snapshot and verifies both readers see complete records. Shell error/retry tests now wait with a bounded observation helper for EOF records rather than equating HTTP response completion with process cleanup. The disconnect test retains its process-exit timeout but removes a redundant post-timeout wall-clock assertion. Initial stress loops exposed that duplicate deadline assertion and the EOF-observation race; after those fixes the full 16-test shell suite passed **20 consecutive iterations (320 test executions, zero failures)**. These are offline results, not live protocol evidence.

Cleanup: retain all seven raw evidence folders above, plus temporary directories `clod-live-verify-{2h_1kwhv,gvi9vxyd,ggbv6wwn,czufadlx,oy3oacl8,ht7dtkxi,lpdxnfbp}` under the host temporary directory. Dedicated Claude project folder suffixes are respectively `clod-live-verify-{2h-1kwhv,gvi9vxyd,ggbv6wwn,czufadlx,oy3oacl8,ht7dtkxi,lpdxnfbp}-workdir` (the CLI encodes underscore as hyphen). Full parent prefix: `~/.claude/projects/-private-var-folders-m4-kmdr4ndn2lv0yxvnlcb6v5gh0000gn-T-`. CLI image temporary files under `/private/tmp/claude-501/<encoded-workdir>/` may also remain. Nothing was deleted.

## Consecutive user messages and declared bare tool calls (original offline fixes)

Resume accepts one or more consecutive trailing user messages after the checkpointed prefix. Their combined blocks must satisfy the pending-tool result set exactly; existing results-before-text ordering remains conservative. The prefix hash excludes the entire trailing user sequence. clod sends **one** stream-json `type:user` record with all blocks in their original order, converting string content into text blocks. The SDK user-message content accepts arrays (as captured by `e-image`); vendored `sdk.d.ts` SDKUserMessage (lines 3657–3675) documents that `shouldQuery:false` is needed to append without triggering a turn and merge with the next queried message. Separate ordinary user lines could therefore start separate CLI turns, so clod merges the input rather than relying on batching. A single trailing message retains its existing string/array representation. This multi-message merge is tested offline and was subsequently live-confirmed in the dated verification section above.

Declared bare tool names are **forwarded unchanged**. All-bare turns may checkpoint under the same gates as prefixed turns; mixed bare/prefixed turns discard with `MixedBareAndPrefixedToolCalls` and replay next turn. With generation complete, error_max_turns still returns a successful tool response to the client. No nonexistent PreToolUse hook is awaited. The later live single-call probe above establishes that the error is a child of the tool record and is removed by fork-at-tool. Mixed bare/prefixed calls were not reproduced, so broad checkpoint safety remains unproved; the native-search fixture and synthetic tests alone do not close that gap.

Tool checkpoints require the last assistant record to contain a forwarded call; later content yields `TrailingContentAfterTool`. When stream events exist, streamed and recorded tool-ID sets must equal. A second message_start suppresses its stream blocks and assistant content, warns, and yields `MultipleMessages`. A Resume invalid_request_error is treated as an unresumable checkpoint: one fresh retry only while uncommitted, otherwise SSE error; both discard the old state. One overall deadline spans both attempts. HTTP body completion (`EndResponse`) is independent of process cleanup (`Done`); the per-key lock remains held until Done.

## Scenario findings

| Scenario / fixture stem | Observed |
|---|---|
| `a-text`, `a-text-resume` | Text success; fork recalled `cobalt`; resume cache reads 752 tokens. |
| `b-tool`, `b-tool-resume` | One lookup; deny held until generation boundary; `error_max_turns`; fork receives real `VALUE_alpha`, answers it; cache reads 1072. |
| `c-parallel` | Three lookup calls in **three separate assistant records**, each containing one tool_use. Last assistant UUID is the checkpoint. |
| `c-second-round`, `c-final` | First fork consumes three results and calls combine; second fork consumes combine result and reports all values. Two-hop lineage works; cache reads 1090 then 1399. |
| `d-thinking` | Enabled budget accepted, answer 391, **text block only**, reported thinking tokens 0. Thinking/signature block pass-through remains unverified. |
| `e-image` | Base64 PNG user block accepted; text answer success. |
| `f-missing-session`, `f-missing-uuid` | Both stale refusals emitted stderr **and** an `error_during_execution` result with the same error text. No assistant records. |
| `upstream-tool`, `upstream-tool-resume` | Same successful tool/fork protocol through custom base URL; upstream body exposes injection contradiction. |

Fork IDs differ from source IDs and appear in stdout `system` init, stream/assistant records, and result `session_id`. Two hops: `bb8f6646-3808-457f-a403-b3f272ee1735` → `48506ed4-e7d6-4ecd-be31-c20a9ab2c7e8` → `36ede346-28b4-4a6b-96e0-271c2d42b4dc`. UUIDs in each prior assistant record successfully resume the corresponding fork.

Source transcripts were hashed read-only under `~/.claude/projects/<encoded-workdir>/<session>.jsonl`. All tool-output equality checks were true. Durable exact evidence for upstream source: before and after SHA-256 **`f1fc35ed16c64ca0dc9ff53156d68bb1037d95e03bf73fb45e4f309907d01627`**. Upstream resumed request history contains the original tool_use followed by real `VALUE_alpha`, **not** the denied tool_result, confirming resume-at+fork removes the denial from the new ancestry. The original denial remains in the source transcript.

Held hook timing (`capture-evidence.json`, `c-parallel`): first hook enters at 2035 ms with generation active; later stream events continue; `message_delta` releases it at 2370 ms; response at 2370 ms. Remaining hooks arrive at 2376/2378 ms with generation inactive and answer immediately. Single-tool hold: 1472→1477 ms. No hold timeout fired.

## Error texts and lifecycle limitations

Live:

- `Reached maximum number of turns (1)` — result.errors; SDK wraps it as described above.
- `No conversation found with session ID: <uuid>` — stderr and result.errors.
- `No message found with message.uuid of: <uuid>` — stderr and result.errors.

Both stale cases use `subtype:error_during_execution`, `is_error:true`, zero turns/usage and no stream events. **Ordering between result, stderr, stdout EOF and exit is unverified** because tee logs do not timestamp cross-pipe events. No claim of stderr-only refusal is made for this version.

Synthetic only (not induced): `g-usage-synthetic.ndjson`: `rate limit exceeded`; `g-auth-synthetic.ndjson`: `API Error: 401 authentication_error`; `g-overflow-synthetic.ndjson`: `Claude Code returned an error result: Prompt is too long`. Source: `meridian/src/__tests__/errors.test.ts` lines 10, 132, 322. These are error-text observations to feed error classifiers, **not fabricated live CLI envelopes**.

## Exact upstream prompt and tools

See `upstream.ndjson` for complete bodies and injected messages. System array:

1. `x-anthropic-billing-header: cc_version=2.1.308.197; cc_entrypoint=sdk-ts;`
2. `You are a Claude agent, built on Anthropic's Claude Agent SDK.`
3. `You are a precise test assistant. Follow the user instructions exactly. Use only the provided tools when requested.`

Items 2–3 have ephemeral one-hour cache controls. Exactly two tools were present, sorted `mcp__oc__combine`, `mcp__oc__lookup`, with the descriptions and schemas declared by the driver. No built-in tools were present. The extra email reminder and environment system message described at the top remain despite attachments disabled. No scratchpad block was observed. Do not equate the CLI's upstream `/api/hello` with an endpoint Polytoken itself calls.

## Polytoken request stability and endpoints

Searched `meridian/src/__tests__/proxy-polytoken*.test.ts` and `meridian/docs/maintenance/evidence/**/*polytoken*`. Tests are constructed requests/mocked SDK behavior, **not recorded consecutive real requests**. Real-client acceptance summaries exist under `docs/maintenance/evidence/polytoken-1339/` (including `live-acceptance.json` and `native-title-contract.json`), but no pair of full consecutive raw bodies was found that establishes byte identity of `system` and `tools`. **That stability check remains unverified; do not fabricate it or infer it from successful resume counts.**

Reference native contract tests explicitly call **POST `/v1/messages`**, including streaming continuations. Real acceptance summaries corroborate working/native-title flows, but are not an exhaustive endpoint trace. Other Polytoken endpoints, if any, remain undetermined. No static endpoint stub requirement is established by this capture.

## Re-capture (live consent required)

Do not rerun to work around the gate without operator review. From repository root:

```sh
chmod +x scripts/capture-claude.sh
/Users/<user>/.local/bin/claude --version
CLOD_CAPTURE_RUN=<unique-run> /opt/homebrew/bin/node scripts/capture.mjs all
# Or select a, b, c, d, e, f separately.
/opt/homebrew/bin/node scripts/capture-upstream.mjs
# In a second terminal:
ANTHROPIC_BASE_URL=http://127.0.0.1:19876 CLOD_CAPTURE_RUN=<unique-run> \
  /opt/homebrew/bin/node scripts/capture.mjs upstream
/opt/homebrew/bin/node scripts/sanitize-captures.mjs scripts/captures/<unique-run>
```

Use a unique run name to avoid overwriting raw evidence; upstream log defaults to append-only `scripts/captures/upstream.ndjson` (override `CLOD_UPSTREAM_LOG`). SDK invocations return error results for capped tool turns, which the driver records rather than treating as missing evidence. Fixtures intentionally omit reliable inter-pipe timing; a future authorized capture needs explicit per-pipe EOF/exit instrumentation to close those checks.
