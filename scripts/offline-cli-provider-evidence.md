# Network-confined Claude CLI scripted-provider evidence (2026-10-27)

## Safety and reproduction

The real installed Claude CLI 2.1.308 was executed only under `/usr/bin/sandbox-exec` with network denied except loopback, isolated HOME/XDG/workdir/TMPDIR, an explicit allowlisted environment, and a fake API key. No live provider calls, subscription authentication, real configuration/credential reads or changes, CLI upgrades, service restarts, reference-checkout imports, or sibling modifications were performed. The scripts do not set `CLAUDE_CONFIG_DIR`.

Before any CLI execution, a separate sandboxed Python child successfully connected to a loopback listener and received `EPERM` (`errno=1`, Operation not permitted) for TCP connections to both `192.0.2.1:443` and `1.1.1.1:443`. A timeout or other connection failure does not satisfy the check. The exact same sandbox profile wraps every CLI child. Missing sandbox support or failed proof aborts before opening/executing the CLI. CLI SHA-256 must equal the previously inspected 2.1.308 binary, `ca923c86ac84b30574a82331a05b6e066c27477a926ceaffe46a845a33e4dd37`.

Run from repository root (output directory must not exist):

```sh
python3 -I scripts/offline-cli-confinement.py
python3 -I scripts/offline-cli-provider.py --output /tmp/clod-offline-cli-new
python3 -I scripts/test-offline-cli-provider.py /tmp/clod-offline-cli-new
```

The harness owns a short-lived loopback HTTP server and all CLI processes, with a 35-second deadline per case, one CLI turn, and a three-request response ceiling per case. A fourth request receives HTTP 429 and fails verification; only exactly one request per case passes. No background services are retained. `shell_service` was not available in the executing tool context; this bounded subprocess/server lifecycle ran in the foreground instead.

Captured artifacts include the exact injected successful response `provider.sse`, actual CLI stdin/stdout NDJSON, stderr, argv, complete upstream request body (including tool schema), and summary. Header/credential capture is deliberately omitted. The harness saves successful preflight proof in `confinement-proof.json`.

## Observed matrix

Successful evidence run: `/tmp/clod-offline-cli-20261027-b` (four requests total, all local). A final rerun after integrating verification uses `/tmp/clod-offline-cli-20261027-c`. Every upstream request advertised only `mcp__oc__probe`, with `items: array<string>`, `count: number`, and `limit: integer`.

| Injected name | Injected arguments | Raw CLI delta | Final CLI assistant input | Tool execution path |
| --- | --- | --- | --- | --- |
| `probe` | array, 2.5, 3 | unchanged | unchanged typed values | `No such tool available: probe`; no MCP tools/call |
| `probe` | array JSON text, `"2.5"`, `"3"` | unchanged | all remain strings | `No such tool available: probe`; no MCP tools/call |
| `mcp__oc__probe` | array, 2.5, 3 | unchanged | unchanged typed values | MCP tools/call receives typed values |
| `mcp__oc__probe` | array JSON text, `"2.5"`, `"3"` | unchanged | array, 2.5, 3 | MCP tools/call receives typed values |

Typed arguments: `{"items":["a","b"],"count":2.5,"limit":3}`. String arguments: `{"items":"[\"a\",\"b\"]","count":"2.5","limit":"3"}`.

The artifact verifier checks upstream schema, exact provider JSON fragment equality with CLI partial-message fragment, assistant normalization, unknown-name errors, and actual MCP tools/call arguments for all four cases. Thus this is paired raw-provider/real-CLI evidence of **name-gated schema normalization after raw partial-message emission**, not a hypothesis based only on static code. It does not establish why any real model emits a bare name or string-valued arguments.

Each CLI exits 1 with `error_max_turns` after the deliberately injected tool turn. That is expected: the local MCP stub reports that execution belongs to the client, and no second model turn is permitted. The harness succeeds only after validating the captured invariants, not merely because an assistant record exists. The first attempted run (`...-a`) failed before any provider request because systemPrompt was encoded as blocks rather than strings; it is not successful evidence. The handshake was corrected to match clod's initialization.

## clod boundary and limitations

The existing offline clod boundary tests were rerun:

```sh
cargo test --offline -p clod-core --test offline_type_evidence -- --nocapture
```

Both passed: actual MCP serialization preserves array/number/integer constraints; streaming and nonstreaming client paths preserve raw string-valued arguments and strip only the known prefix. No actual clod server/client roundtrip was executed for this new capture; the CLI experiment plus existing synthetic clod tests are separate evidence layers. No production protocol/checkpoint behavior was changed.

All generated isolated CLI homes/transcripts are confined to the dedicated `/tmp/clod-offline-cli-20261027-{a,b,c}` directories. They were left for operator cleanup; no real CLI transcript directories were touched. Raw captured request bodies include injected CLI system text and temporary paths; sanitize before exporting them.
