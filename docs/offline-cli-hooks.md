# Offline PreToolUse interception experiment

## Conclusion

On the locally installed Claude CLI **2.1.314**, an SDK `PreToolUse` hook does **not** intercept an unknown bare tool name before the CLI emits its local unavailable-tool result. Broadening clod's matcher from `mcp__oc__.*` to `.*` or the explicit bare name `probe` cannot fix this behavior.

A two-tool fake provider message reproducibly becomes assistant A → local error result A → assistant B. The saved transcript's B `parentUuid` is result A's UUID, not A's. The existing `ToolResultBeforeLastToolCall` checkpoint rejection must remain.

## Scope and safety

This was real CLI execution with synthetic local provider responses, not a live model test. No CLI upgrade, unrestricted CLI invocation, production change, commit, credential/settings read, real HOME CLI use, meridian/sibling modification/import/build, or service restart was performed.

Static metadata: `~/.local/bin/claude` resolved to `/Users/titanous/.local/share/claude/versions/2.1.314`, 278848096 bytes, SHA-256 `21d6e7b9a6f07b4cfcb7df13760b3b72aa25e084e13a5354413382987ba832ed`. Version is established from the installed version path, not an unrestricted `--version` execution. The existing 2.1.308 harness and tested-version declaration were not changed. A separate experiment requires these exact 2.1.314 bytes.

Every CLI child ran under `/usr/bin/sandbox-exec` with the reviewed `offline-cli-confinement.py` profile. A fresh preflight proved loopback connects and both 192.0.2.1:443 and 1.1.1.1:443 fail with OS `EPERM`, before CLI execution. HOME, XDG directories, TMPDIR and workdir were dedicated capture directories; environment was an allowlist with a fake API key and local provider URL, settings sources disabled, updater/telemetry disabled, no built-in tools, strict SDK MCP configuration. The network profile allows loopback generally, not only the fake provider's port; it is network confinement, not general filesystem isolation. It does not cryptographically prove that a binary cannot attempt an absolute filesystem read. No real settings or credential paths were supplied or inspected by the experiment.

Each corrected case had a 35-second deadline, two served POST responses maximum, and kills the confined child on a third POST. All four completed with exactly two POSTs, exit 0, no timeout and zero MCP `tools/call` requests. All POST paths were `/v1/messages` paths. No real provider request was authorized or made by the harness.

## Cases

All four initialize responses were `success`. Every initial SSE response contained two `tool_use` blocks in **one provider message**, IDs `toolu_hook_0` and `toolu_hook_1`, input `{}`. SDK MCP server `oc` advertised only tool `probe` (native name `mcp__oc__probe`). Hooks were supplied in the initialize request as `hooks.PreToolUse = [{matcher, hookCallbackIds:["deny_probe"], timeout:5}]`.

| Provider tool names | Matcher | Hook callbacks | Local results |
|---|---|---:|---|
| `mcp__oc__probe` twice | `mcp__oc__.*` | 2 | hook denial twice |
| `probe` twice | `mcp__oc__.*` | 0 | unavailable tool twice |
| `probe` twice | `.*` | 0 | unavailable tool twice |
| `probe` twice | `probe` | 0 | unavailable tool twice |

For the prefixed control, each callback was held for 0.3 seconds, then answered with `hookSpecificOutput: {hookEventName:"PreToolUse", permissionDecision:"deny", permissionDecisionReason:"Offline hook denied execution"}`. Its own tool result was absent while held; the provider request count remained one before each deny. Results contained `PreToolUse:mcp__oc__probe hook error: Offline hook denied execution`, and there was no MCP execution. Thus registration and pause/deny behavior work for resolved tools; the negative bare cases are not explained by hook initialization failure.

For bare catch-all, A UUID `1fb553d6-d3cf-4de4-8e8f-f1b514643854`; result A UUID `ce165469-23ac-4b31-94d5-d313bf581d99`, parent A; B UUID `e6a55b5e-bf1d-4588-b198-c18a58e3121a`, parent result A. All three bare cases independently have that parent-chain shape. The prefixed control B's parent is A, not result A.

Static corroboration in the pinned binary: embedded JS at byte 238619419 contains the queued-tool path's `No such tool available` result; it appends an already-completed tool result and returns before normal execution. Another rejection path appears at byte 233603385. Embedded hook initialization schema at byte 222682265 describes `hookCallbackIds`; registration conversion appears at 258564382. Runtime evidence establishes the practical ordering: unknown lookup/rejection happens without invoking PreToolUse; hooks can pause/deny resolved calls only.

## Reproduction and evidence

From repository root on the same macOS host with the exact reviewed bytes:

```
python3 -I scripts/offline-cli-hooks.py --output scripts/captures/offline-hooks-new-run
python3 -I scripts/test-offline-cli-hooks.py scripts/captures/offline-hooks-new-run
```

Passing capture: `scripts/captures/offline-hooks-2.1.314-v2/`. It contains confinement proof, binary metadata, per-case argv/stdin/stdout/stderr, exact fake provider SSE, captured provider request bodies, summaries and isolated CLI transcripts under each case's `home/.claude/projects/`. Captures are ignored local evidence; requests contain CLI-injected prompts and local paths and should not be exported wholesale. Leave cleanup to the operator.

An earlier capture at `scripts/captures/offline-hooks-2.1.314/` is **incomplete**, not passing evidence. The final fake text SSE lacked a text delta. The CLI requested continuation and retried ceiling 429s (8–9 local attempts per case; only two responses served), then was killed at the deadline. The corrected SSE includes a text delta, and a third POST now immediately kills the child rather than allowing retries. This error never bypassed OS confinement. The artifact verifier initially needed a system-Python compatibility correction (`zip(strict=True)`) and a per-tool rather than global pause assertion; its final run passes all four saved cases without reexecuting CLI.

## Fix implications

There is no viable minimal core change consisting of widening the PreToolUse matcher: the callback does not exist for unknown tools, so clod cannot hold or deny it before rejection. Do not remove `ToolResultBeforeLastToolCall` at `crates/clod-core/src/exchange.rs:602`, synthesize a hook callback, or save B as though result A were not its ancestor. The CLI-generated result is an internal history mutation even when downstream clients own execution.

Alternative work requires a separate design and evidence: ensure emitted names resolve before execution (align model-facing naming/replay; or investigate supported explicit aliases), or change CLI behavior upstream to invoke a pre-lookup callback or avoid persisting local results between batch calls. This experiment does not validate aliases, argument normalization, resume from a changed checkpoint, or model compliance with naming instructions. No exact alias/core patch is justified by these results alone.

Required regression coverage for any future fix: known prefixed callback registration and hold/deny; unknown bare catch-all and explicit matcher yielding no callback; two same-message bare calls with result A ancestor of B remaining uncheckpointable; mixed known/unknown batches; downstream real tool-result continuation without duplicate local results; existing safe prefixed resume unaffected. Keep protocol/checkpoint policy in clod-core, with the shell only observing/executing actions. This investigation added no production changes and ran no workspace build or live smoke.
