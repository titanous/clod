# Real Polytoken request harness

This runs the **installed Polytoken binary**, not a replacement client, against a
scripted local Anthropic-compatible HTTP server. Python 3 standard library only;
use `-I` to avoid Python user packages and startup configuration. No live model
calls are needed or made by the default scenarios.

## Run

From the clod repository root:

```sh
command -v polytoken
polytoken --version
python3 -I scripts/polytoken-harness/test_harness.py
python3 -I scripts/polytoken-harness/run.py --output scripts/polytoken-harness/output
python3 -I scripts/polytoken-harness/analyze.py \
  scripts/polytoken-harness/output/exec.ndjson \
  scripts/polytoken-harness/output/daemon.ndjson
```

The recorded fixtures were produced with `~/.local/bin/polytoken`,
**polytoken 0.9.2**. No binary build was necessary. The runner accepts `--binary`,
`--scenario exec|daemon|both`, `--timeout SECONDS`, and `--output DIRECTORY`.
It does not build or modify Polytoken. It creates no Rust crate and changes no
workspace dependencies.

Outputs are `exec.ndjson`, `daemon.ndjson`, and corresponding `.run.json` files
with binary version, scrubbed commands, exit codes, CLI output, and daemon prompt
acknowledgments. The ignored `output/` directory is for local reruns. Checked-in
recordings live at:

- `crates/clod-core/tests/fixtures/polytoken/exec.ndjson`
- `crates/clod-core/tests/fixtures/polytoken/daemon.ndjson`

`mock.py` also runs standalone with `--script response-blocks.json --recording
requests.ndjson`; it prints its ephemeral loopback port. A script is a JSON array
of responses, each an array of Anthropic content blocks (`thinking`, `text`,
`tool_use`). The runner's `scenario_script()` supplies the default scripts.
Unscripted message requests return HTTP 400 instead of silently repeating a reply.
Title requests exposing `_infer_title` are separately labeled and do not consume
the main response sequence. Every request, including unexpected endpoints, is
recorded before responding. `/v1/models` has a minimal static response.

## Isolation and redaction

Every scenario creates a fresh temporary HOME, XDG_CONFIG_HOME, XDG_DATA_HOME,
XDG_CACHE_HOME, explicit `--config-dir`, and working directory containing only
`hello.txt`. It launches Polytoken with that working directory, not clod, so no
repository or user project configuration is loaded. It removes inherited
`POLYTOKEN_*` (including all path overrides), Anthropic/OpenAI/AWS credentials,
OTEL and Sentry variables, and disables update checks. The only configured model
is `claude-test` on the loopback mock, using static `test-key` authentication.
The daemon credential is read only from the temporary tree and is never saved.
The daemon is terminated in `finally`, the mock is shut down, and the temporary
tree is deleted. Exec/startup and each daemon turn have bounded timeouts.

NDJSON contains method, path, lowercase headers, full parsed JSON body, request
ordinal, main/title label, and SHA-256 hashes of the **original wire JSON value
slices** for `system`, `tools`, and `model`. Hashes distinguish whitespace changes
that parsed-JSON equality would miss. They describe pre-scrub bytes and therefore
are not generally reproducible from the scrubbed bodies. Source-path examples in recorded system text have also been replaced with placeholders without changing those original hashes. The raw body formatting
outside these fields is not preserved. Headers containing authorization,
API-key, cookie, or token names are redacted. Run paths become `<RUN>`, the mock
port becomes `<MOCK_PORT>`, and identity values become equality-preserving
`<IDENTITY_N>` placeholders. These placeholders are local to each recording;
`IDENTITY_1` in exec is not evidence of the same real session as daemon.
Content-Length retains the actual pre-scrub request byte count.

## Scenarios and observed stability

A (`exec`): first response contains thinking with signature, assistant text, and
`file_read({"path":"hello.txt"})`; the second contains text and
`exec_structured_output({"output":"harness complete"})`, ending exec.
The built-in read succeeds with `1 | hello from the harness\n`.

B (`new --no-attach`): authenticated POST `/prompt` followed by GET `/state`
until idle. Prompt one uses the same read-only tool round, then `end_turn`;
prompt two is a separate top-level prompt in the **same session**, answered with
text and `end_turn`. There are three main provider requests total. Installed
0.9.2 omits `turn_in_flight` when false, so the runner treats absence as idle.
It reports assistant text from `most_recent_assistant_text`.

| Comparison | system | tools (including order) | model | Session header | Conversation header |
|---|---|---|---|---|---|
| exec request 1 → 2, same turn | wire-identical | wire-identical | wire-identical | identical | identical |
| daemon request 1 → 2, same turn | wire-identical | wire-identical | wire-identical | identical | identical |
| daemon request 2 → 3, across prompts | wire-identical | wire-identical | wire-identical | identical | identical |

The model is `claude-test`; both identity headers have the same value within each
scenario. There are **no cache_control fields anywhere** in these recordings.
Date/context text and tool order do not change between the measured requests.
These short same-day runs do not prove stability across midnight, reloads, facet
changes, model changes, tool discovery, or longer sessions.

The exact selected-body diffs are empty. The only header diffs are:

```diff
# exec 1 -> 2
- "content-length": "85971"
+ "content-length": "86366"
# daemon 1 -> 2
- "content-length": "89356"
+ "content-length": "89751"
# daemon 2 -> 3
- "content-length": "89751"
+ "content-length": "89955"
```

`analyze.py` compares the saved fixtures and asserts successful tool execution
and preservation of thinking, signature, and tool_use.

### What changes in messages

The tool continuation appends these exact blocks after the existing prompt:

```json
{"role":"assistant","content":[
  {"type":"thinking","thinking":"I will read the harmless fixture.","signature":"harness-signature-v1"},
  {"type":"text","text":"Reading the fixture."},
  {"type":"tool_use","id":"read_1","name":"file_read","input":{"path":"hello.txt"}}
]}
{"role":"user","content":[
  {"type":"tool_result","tool_use_id":"read_1","content":"1 | hello from the harness\n","is_error":false}
]}
```

The thinking signature is synthetic; it is parsed and echoed, not cryptographically
validated against Anthropic. The daemon's third request retains those blocks and
appends:

```json
{"role":"assistant","content":[{"type":"text","text":"The fixture says hello from the harness."}]}
{"role":"user","content":[{"type":"text","text":"This is the second top-level prompt. Reply briefly."}]}
```

Exec also injects a user-role `<system-reminder>` about its active facet declaring
unavailable `ask_user_question` and `switch_facet` tools. It is retained in the
continuation, not moved into the `system` field. This is observed binary behavior,
not a harness-generated reminder, and does not prevent `file_read` succeeding.

Observed provider endpoints: only **POST `/v1/messages`** (2 exec + 3 daemon).
No GET `/v1/models` or `_infer_title` request occurred. Three loop responses in the
daemon did not produce an observed detached title call before termination; this
is not evidence that longer sessions never infer titles. Daemon control calls
(`/prompt`, `/state`, `/terminate`) are separate from provider traffic and are not
in the provider NDJSON.

## Later smoke testing through clod

```sh
python3 -I scripts/polytoken-harness/run.py --scenario both \
  --provider-url http://127.0.0.1:8080 --output scripts/polytoken-harness/output-clod
```

**An explicit provider override may make live calls**, depending on clod's upstream
configuration. It was not used for the captured fixtures. Configure clod to forward
to a separately running `mock.py` for a still-offline proxy test. The runner keeps
its isolation but sends directly to the override: its own mock sees no requests,
so runner NDJSON will be empty in override mode. Capture requests at clod or the
upstream standalone mock for that smoke test. The runner retains the synthetic
model name and test key; arrange clod routing accordingly. Script-consumption
assertions apply only in default local-mock mode.

## Verification and limitations

Commands actually run: installed-binary lookup/version; both-scenario runner
(several times while resolving the omitted idle field); the two standard-library
tests; analyzer against both saved fixtures; and scans showing no temporary-path
or `/Users/` leaks in saved NDJSON. Both scenarios pass with expected request counts
and successful file-read continuation. No live provider override was tested.
No workspace crate was added; `cargo test --workspace` was not run because these
are Python-only additions and another agent was editing Rust source concurrently.
No external checkout or core source files were modified.
No commit was made.

## Context changes

Additional installed-0.9.2 runs use only the local scripted provider, with the
same temporary HOME/XDG isolation. Run `--scenario facet`, `goal`, or `reveal`;
`--scenario all` includes all five scenarios (`both` retains its old meaning).
New recordings and scrubbed run metadata are in
`crates/clod-core/tests/fixtures/polytoken/{facet,goal,reveal}.{ndjson,run.json}`.
The analyzer compares system/tool/header values, names added/removed/changed
tools, and detects order-only changes.

### Trigger and measured changes

The harness sends authenticated `POST /facet` with `{"facet":"plan"}` or
`{"facet":"execute"}` while idle, then sends the next `/prompt`. The recorded
provider requests show the system and tool changes below. No TUI or slash-command
simulation is needed. The starting facet in these runs is execute.

| Recording / adjacent requests | system | tools | model / both identity headers |
|---|---|---|---|
| facet 1 → 2: first prompt's file-read continuation | identical | identical | identical |
| facet 2 → 3: execute → plan, prompt two | changed | changed | identical |
| facet 3 → 4: plan → execute, prompt three | changed back | changed back | identical |
| goal 1 → 2: file-read continuation | identical | identical | identical |
| goal 2 → 3: goal set and paused before prompt two | changed | identical | identical |
| reveal 1 → 2: file-read continuation | identical | identical | identical |
| reveal 2 → 3: tool search/reveal within first turn | changed | changed | identical |
| reveal 3 → 4: next top-level prompt | identical | identical | identical |

All unchanged fields above are original-wire-value byte-identical. No measured
change was order-only. No same-name tool definition changed on facet switches:
execute → plan adds `edit_plan`, `handoff_plan`, `write_plan`; removes
`file_edit_search_replace`, `file_write`, `shell_monitor`, `switch_facet`,
`tool_flow`. The reverse switch reverses precisely those changes. Request four's
system and tools equal request one's, not merely approximately. The long system
diff replaces matching tool-library/example/available-tool entries, removes
writing/waiting/tool-flow/facet-switch instructions, adds plan-writing and
read-only/side-effect/todo/intent/evidence/plan-artifact/review rules, and changes:

```diff
-You are in execute facet. Implement the plan systematically.
+You are in plan facet. This is a read-only planning and investigation mode.
```

The goal scenario sends `POST /goal` with
`{"summary":"Report the fixture contents, then wait for instructions."}` followed
by `POST /goal/pause {}`. Pausing preserves goal context without permitting
unbounded automatic goal-driver requests. An initial unpaused probe triggered
an extra automatic request and correctly failed script-consumption validation;
that failed probe is not a saved fixture. Exact system insertion (session path
scrubbed):

```diff
+
+## Current Goal
+
+Summary: Report the fixture contents, then wait for instructions.
+Lifecycle: Paused
+File: <DAEMON_SESSION>/goal-001.md
+Terminal reason: none
```

The reveal scenario sets the isolated model's `tool_loading = "native_deferred"`
and returns `tool_search({"query":"file_write","max_results":1})`. Its successful
result is `- file_write: Write content to a file in the project`. No live MCP or
other service is needed; the file-write tool is revealed, not executed. The tool
name already exists as a deferred stub, so the analyzer correctly reports
`changed: ["file_write"]`, not an added tool. Exact schema changes, with the
unchanged description omitted here:

```diff
 "input_schema": {
   "type": "object",
-  "properties": {}
-},
-"defer_loading": true
+  "properties": {
+    "path": {
+      "type": "string",
+      "description": "Path to the file to write. Relative paths resolve against the current working directory; absolute paths inside the project are also accepted."
+    },
+    "content": {
+      "type": "string",
+      "description": "The complete file content to write. Generate the full file text and pass it here."
+    }
+  },
+  "required": ["path", "content"]
+}
```

The tool moves from its deferred position to the eager group. System adds its
example `{"path":"tmp/example.txt","content":"example contents\\n"}`, adds its
full description to Available Tools, and removes that description from Deferred
Tools. All other tool definitions remain equal. The complete precise positional
changes can be compared in the saved recordings. No separate plan-document scenario was needed:
goal setting and both facet directions were achieved offline.

### Time-dependent system content

The recorded system text contains a `Date` field, plus `Session`, `CWD`, and
`Daemon PID`; those stay constant within each measured daemon. No wall-clock
seconds or request timestamps were found in system. The analyzer checks the run
date and lists ISO dates and temporal/session/PID lines for every request.
These are same-day measurements, not an actual midnight experiment. They do not
establish date-refresh behavior in a long-running daemon. Clod's key includes the
supplied system bytes, so differing system text selects a different key.

### Core hash result and recommendation

`polytoken_followup_hash_matches` is enabled and passes. It parses all three
baseline `daemon.ndjson` requests with `request::parse`, checks keys for both
adjacent pairs, and reconstructs request history plus the mock's returned
assistant blocks in the same shape as `exchange.rs`. Both reconstructed prefix
hashes equal the next request's prior-history hash, across the tool round and
across top-level prompts. Thinking/signature remain in the reconstructed blocks;
the existing canonical hash excludes them. No echo/reconstruction discrepancy
was found, and the assertion was not weakened.

**Recommendation: keep the existing key** (conversation + model + system + full
tool definitions). Facet changes, goal changes, and tool reveal demonstrably
force new keys, including reveal within a turn. This is a real replay cost, but
these changes also alter instructions and available schemas: a conversation +
model-only key would resume an existing process with stale system/tool context
unless clod explicitly synchronizes that context on resume. These recordings
alone do not establish that synchronization is safe or implemented. Preserve
correctness now; optimize by propagating context updates before relaxing the
key, rather than ignoring material changes. Switching back can find an old key,
but prefix validation must still reject a stale history checkpoint.

Verification: three harness unit tests pass; new scenarios consumed exactly
4/3/4 main responses; analyzer continuation assertions pass; `cargo test -p
clod-core` passes (33 unit + 2 dependency + 52 exchange tests, zero ignored).
No live model calls or commit were made during that verification. No external
checkout, shell crate, or root Cargo files were edited. The only core source edit
was the session test.
