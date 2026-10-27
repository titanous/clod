# Private upstream response capture

`scripts/capture-upstream.mjs` forwards HTTP response payload bytes without decoding content compression. HTTP transfer framing is handled by Node and is not captured. Captured chunks are transport observations, not SSE event boundaries or provider write boundaries.

The existing `CLOD_UPSTREAM_LOG` request log remains best-effort redacted. Each forwarded request now has a monotonic `request_id` (unique within that recorder process), alongside `cli_run` from `/__run/<hex>/...` and the existing messages-ceiling `index`. Non-message requests also get unique request IDs. Pair request/response evidence using `request_id` within one recorder log; use `cli_run` to associate it with the dedicated CLI stdout capture. IDs restart with the process; do not merge unrelated recorder logs without preserving provenance.

Responses, including successes, append to `${CLOD_UPSTREAM_LOG}.responses.ndjson` by default (default full path `scripts/captures/upstream.ndjson.responses.ndjson`). Override with `CLOD_UPSTREAM_RESPONSE_LOG`. Each response has:

- `response_start`: `request_id`, `cli_run`, `index`, elapsed `t_ms`, HTTP `status`, and allowlisted `headers` (content type/encoding/length, retry-after, request-id, anthropic-request-id). Authentication and cookie headers are not recorded.
- `response_chunk`: the same identity, elapsed `t_ms`, captured-byte `offset`, and `bytes_base64` containing actual payload bytes, including compression if present. Concatenate decoded chunks in append order per request, then decompress according to the recorded encoding to inspect SSE. No UTF-8 conversion or redaction occurs in these bytes.
- `response_end`: the same identity, elapsed `t_ms`, `received_bytes`, `captured_bytes`, `truncated`, `complete`, and a nullable mechanical `error_code`. A missing end record means capture was interrupted; it is not proof of provider completion. `complete` describes the HTTP forwarding pipeline, not model/SSE semantic completion or CLI consumption.

`CLOD_CAPTURE_RESPONSE_BYTES` defaults to 16777216 (16 MiB) per response. Beyond this prefix, forwarding continues unchanged and byte counts continue, but payload capture stops. A truncated capture is not sufficient to compare the entire response. The JavaScript API accepts `responseDestination` and `responseLimitBytes` with the same defaults. Records append synchronously without accumulating the entire successful response in memory; slow disk can delay forwarding. The existing error-body summary also has bounded compressed input and decompressed output. The request ceiling still bounds message calls, not all recorder traffic or total disk usage.

## Privacy and handling

**The response sidecar is raw private evidence, not a sanitized fixture.** Exact bytes can contain prompts, generated account information, private paths, email addresses, tool arguments/results, or secrets echoed by the provider. Base64 is encoding, not protection. Request-body redaction is also best-effort and does not make the request log safe to publish. Only response header names are allowlisted; raw body content is deliberately untouched to preserve evidence.

New sidecar files are created mode 0600; existing file permissions are not changed. Keep all raw output in `scripts/captures/`, which is ignored by git, or an equivalently private ignored location. Custom output paths are the operator's responsibility. Never export raw sidecars or add them to fixtures. Produce separately sanitized excerpts with provenance and retain truncation/completion limitations. Do not infer cache tokens from byte counts or sum duplicate usage events.

Offline checks (local scripted HTTP only):

```sh
node scripts/test-upstream.mjs
node scripts/test-upstream-error.mjs
node scripts/test-upstream-response.mjs
```

These tests do not authenticate or make live provider calls.
