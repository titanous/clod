# Offline tool-name and input-type evidence

## Scope and safety

No real Claude CLI, provider request, installed CLI change, authentication/settings change, service restart, or reference-checkout import was performed. Only pure `clod-core` tests ran, using Cargo offline mode. The new integration test constructs synthetic CLI NDJSON in memory; it is not a newly captured provider response.

The existing `scripts/capture.mjs` is unsuitable here: it imports SDK/Zod from `meridian/node_modules` and calls the real SDK query path. `scripts/capture-upstream.mjs` is a forwarding proxy whose default target is the live API. `scripts/live-verify.py` selects real authentication HOME for its CLI child. None of these inspected paths establishes OS-enforced egress isolation. The Polytoken mock harness isolates HOME/XDG and configures a scripted provider, but does not establish network confinement for the real Claude CLI. Consequently the installed CLI was not launched.

## Reproduction

From repository root:

```sh
cargo test --offline -p clod-core --test offline_type_evidence --test tool_batches --test bare_tools --test live_resume -- --nocapture
```

Observed: 13 passed, 0 failed, 0 ignored. No ignored live tests were selected.

New test: `crates/clod-core/tests/offline_type_evidence.rs`.

### MCP schema boundary

A real `Exchange` receiving a synthetic `mcp_message/tools/list` emits this tool through `Action::WriteStdin`:

```json
{"name":"probe","description":"Offline type probe","execution":{"taskSupport":"forbidden"},"inputSchema":{"$schema":"http://json-schema.org/draft-07/schema#","type":"object","required":["items","count"],"properties":{"items":{"type":"array","items":{"type":"string"}},"count":{"type":"number"},"limit":{"type":"integer"}}}}
```

This verifies clod's actual MCP schema serialization: array, number and integer constraints survive, and draft-07 `$schema` is added. It does **not** verify how a newly executed CLI advertises that schema upstream.

Separately, existing sanitized capture `crates/clod-core/tests/fixtures/upstream.ndjson`, records 3 and 5, contains the actual historical upstream schemas: `mcp__oc__combine` advertises `values: {type: array, items: {type: string}}`, required `values`, and draft-07 `$schema`; `mcp__oc__lookup` advertises required string `key`. Record 5 carries prefixed historical tool_use with `input: {key: alpha}` and the real client result `VALUE_alpha`. This capture has no numerical probe and no raw provider response body. Records 1/4 are HEAD `/api/hello`; record 2 is a separate title-generation request, demonstrating ancillary requests in that CLI run.

### Exact input transformations

The new test exercises a two-call batch for each cross-product of streaming/nonstreaming and CLI tool name `probe`/`mcp__oc__probe`:

1. `{ "items": ["a","b"], "count": 2.5, "limit": 3 }`
2. `{ "items": "[\"a\",\"b\"]", "count": "2.5", "limit": "3" }`

For both names, the client sees tool name `probe`: the known prefix is removed; an already bare name remains bare. Call IDs remain distinct. For streaming, input JSON deltas fragmented inside tokens concatenate **byte-for-byte** to the original text, including whitespace. For nonstreaming, the corresponding parsed JSON values are unchanged. Input 1 retains array/number/integer values; input 2 retains strings, including JSON-looking array text. There is no array/numeric coercion or schema validation at this boundary.

The existing `tool_batches` tests additionally verify string-encoded array preservation, three-call typed batches, escaped/newline scalar strings, client-result propagation, and checkpoint outcomes:

- Prefixed batch: checkpoint commits; all three real client results appear in resumed stdin.
- Bare batch with CLI error results interleaved before subsequent tool records: checkpoint discards with `ToolResultBeforeLastToolCall`; follow-up is fresh replay containing every real client result, not CLI error placeholders.
- Single bare call or bare errors only after all tool records: existing safety gates allow checkpoint.
- Mixed bare/prefixed turn: discards with `MixedBareAndPrefixedToolCalls` (`bare_tools` test).

## Evidence boundary / remaining uncertainty

Existing batch regressions synthesize the interleaved ordering based on prior observations; they do not execute a CLI or independently re-prove ancestry in its saved transcript. `fixtures/live/bare-lookup.ndjson` is a documented sanitized real single-call stdout capture, with explicit provenance in `fixtures/live/README.md`; it is not a numeric or string-array reproduction. The replay suite verifies captured live tool output and follow-up planning, not live provider behavior.

`docs/protocol.md` reports historical string-encoded arrays already present in CLI transcript inputs, but that is not evidence of whether the model produced strings or the CLI transformed a typed provider response. The inspected upstream recorder captures request bodies and error responses, not successful raw provider SSE. Without a safely confined CLI and scripted provider (or paired historical successful provider SSE plus CLI stdout), that attribution remains unproved. The offline tests establish only that clod does not introduce the conversion after CLI stdout and that its MCP schema retains the declared types.
