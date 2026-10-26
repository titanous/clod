# Polytoken through clod: Anthropic prompt caching

For the recorder's paired raw response format, bounds, and privacy precautions, see [Private upstream response capture](upstream-capture.md).

## 1. Summary

Polytoken's **per-session system values, deferred-tool exposure, facet switches, and goal insertion** change cacheable prefixes. Stable requests recover caching. The amount read at a change depends on what is still warm within the one-hour TTL: a zero-read result is the observed worst case, not an inevitable outcome of every switch.

Several apparent cache problems were instead clod/CLI behavior: resumed CLI sessions ignore a new systemPrompt, fresh replay rewrites history, user images previously forced replay because of message shape (now fixed offline), and a rejected 1×1 image was silently stripped and retried by the CLI. Conversation-key mode was removed; strict keys are the only supported behavior. No new live calls were made for this final review.

## 2. Setup, evidence and request ledger

Versions: Polytoken **0.9.2**, Claude CLI **2.1.308**, clod **0.1.0**. Chain: isolated temporary Polytoken HOME/XDG/config/workdir → incoming recorder → clod → real Claude CLI with real HOME and no CLAUDE_CONFIG_DIR → local recording proxy → Anthropic. Only dedicated measurement-project transcripts were read. No settings, credentials, Keychain entries, other-project transcripts, deletions or commits were changed.

**Request ledger:** core batch **88** (Haiku60 / Opus28) + strict-final **15 Haiku** + finish **17** (15/2) + remaining **10** (5/5) + image64 **3 Haiku** + clod restart **4 Haiku** + effort **2 Haiku** = **139 requests (Haiku104 / Opus35)**. The **18 earlier broken-capture requests without usable CLI usage** are noted separately; do not use them for cache conclusions or add nonexistent usage to the tables.

Numbers come from the CLI's raw `message_start.usage`; message_start, message_delta and result usage are not summed. Full raw recordings are gitignored under `scripts/cache-measurements/<run>/`: `incoming.ndjson`, `upstream.ndjson`, `usage.ndjson`, `cli-*.ndjson`, `stdout-*.ndjson`, setup and harness metadata. Upstream bodies are parsed JSON comparisons, not preserved original wire slices. Incoming recordings retain subtree wire hashes. Attribution is exact where tool lists/system text match; allocating token loss to transformed history is sometimes inference.

Generated evidence: [matrix-table.md](cache-measurements/matrix-table.md) and [matrix-summary.json](cache-measurements/matrix-summary.json). They cover all37 directories with setup and usage logs, including explicitly labeled offline runs: **131 usage rows /132 upstream attempts**, not a paid-request ledger. Rows record plans/reasons, read/creation/uncached counts, actual model, cache markers, retry indexes and cleanup paths. The failed image attempt has no message_start; request4 alone owns its successful usage. Older18 broken-capture calls have no usable usage log.

## 3. Prefix structure and TTL

Anthropic's effective order is **tools → system blocks → messages**.

- **Breakpoint1:** tools + billing line + SDK identity. Warm reads: **12,885 tokens** for the36-tool eager set; **8,002** for the37-tool deferred set.
- **Breakpoint2:** end of the supplied system block, approximately **31.3–31.7k tokens** in the eager runs. Recorded system text includes Session, CWD, Date and Daemon PID. Separate recorded runs differ in model+tools+system[1:]; their first supplied-system difference is around character40,639 at Session. A fresh session commonly reads only breakpoint1 and creates about18.8k.
- Recorded goal insertion is near character44k, before Available Tools. The measured change reduces reads from~31k to12,885 and creates~19k.
- Cache markers and creation breakdown show **ephemeral1h**,5m0. The study did not wait out TTL deliberately. `strict-final-canary` is **TTL-expired natural cold** (read0/create31,700), not an independent cache-isolation control: it ran around20:17 after the prior batch around19:16. Other new zero-read cases likewise followed >1h since the relevant tools/model were used.

Provider cache is shared, but the31,078–31,079 reads on facet return, goal clear and clod restart are reuse from **earlier in the same session**, within TTL: their system[2] equals that session's own requests1–2. They are not cross-run history reuse. Earlier conversation-mode11,824 facet and8,822/8,002 reveal reads did carry over shared tool prefixes warmed by the preceding strict run; that was not a mode advantage.

## 4. (a) Scenarios that cause a cache break (Polytoken-caused)

Expected-frequency ordering is a judgment, not a usage-frequency measurement. All numbers below are **cache reads**. Fresh requests additionally lose native history caching because clod renders one replay user message. Exact counterfactual prefix-token counts beyond measured breakpoints remain unknown.

| Trigger | Invalidated prefix and before → at → stable after | Evidence and certainty |
|---|---|---|
| **New session / per-session orientation** | Supplied system after breakpoint1 onward. Warm fresh sessions read12,885/create~18.8k; TTL-expired natural cold read0/create31,700. No same-session change triplet. | Measured differing per-session bytes and usage across fresh runs; attribution of exact token split inferred. |
| **Deferred-tool reveal** | Tool list onward. Haiku **25,790 →0 →26,414**; Opus **25,786 →0 →26,063**. Zero is worst case with expired tool-set cache. | Measured: `finish-reveal-claude-haiku-5-5`3/4/5; `remaining-reveal`3/4/5. Actual prefixed tool_search committed; changed schema selects new clod strict key. Change creation26,414/26,412, input2. |
| **Facet switch: execute→plan** | Tools onward. **31,341 →0 →33,906**; change creation34,257/input2. Another session with plan tools warm retained **11,824**, so full miss is not guaranteed. | Measured `strict-final-facet`2/3/4. Exact tool-list difference; token allocation among tools/system/replay inferred. |
| **Facet switch: plan→execute** | Tools change; earlier same-session execute prefix is reusable. **33,906 →31,078 →31,655**, creation928/input2 at change. Native history still replays. | Measured same run4/5/6; at-change system[2] equals this run's1–2. TTL-dependent recovery, not cross-run reuse. |
| **Goal set then paused** | Supplied system after breakpoint1 and history. **31,341 →12,885 →31,582**; creation19,050/input2 at change. | Measured `strict-final-goal`2/3/4. Set/pause combined; isolated active/pause lifecycle costs not measured. Replicate `strict-final-goalclear`2/3/4: **31,343 →12,885 →31,585**, labeled **goal insertion replicate; clear not reached**. |

## 5. (b) Not a break / measured fine

- **Billing cc_version suffix:** not part of effective cache matching in the observed cases. Original strict facet return read31,078–31,080 across `.810`→`.24e`. Generated divergence labels ignore this suffix; a later supplied-system change is system/2, not system/0.
- **Goal clear, within TTL:** `finish-goalclear`4/5/6 reads **31,583 →31,079 →31,570**, change creation841/input2. Cleared system matches this session's earlier pre-goal system; history replays, but the earlier system prefix remains warm. Clearing after TTL may not retain that prefix. Goal insertion replicate in strict-final-goalclear is not evidence for clear.
- **64×64 user image:** `image64-diagnosis`1/2/3 reads **12,885 →31,228 →31,763**, creates18,959/888/375, input2/2/4. One image-bearing upstream attempt succeeded first time; image bytes match incoming → clod stdin → upstream. No Polytoken system/tool change or key change. The Fresh image plan is a **clod message-shape limitation**, below; this is not a Polytoken cache break.
- **Clod restart:** `last-clodrestart`2/3/4 reads **31,342 →31,079 →31,514** (change create788/input2). Fresh map requires replay; same-session system cache remains warm. This loses history, not the shared system prefix; see clod issues.
- **Model switch, inherent rather than Polytoken-caused:** `finish-model`2/3/4 reads **31,342(Haiku) →0(Opus) →31,568(Opus)**; change creates31,918/input2. Recorded system and tools are identical; the model changed and no cache read was recorded on the first Opus request. This does not demonstrate cross-model cache reuse.
- **Environment message hash:** the CLI Environment message differed first→second request while resumed reads remained high. A changed message hash alone does not demonstrate cache loss.
- **IncompleteRead:** teardown recorder artifact, not an additional API generation or cache failure.

## 6. (c) clod / CLI issues

### CLI ignores systemPrompt on resume

All26 reviewed original resumes used their originating fresh system[2], despite clod sending initialize systemPrompt again; tools were supplied fresh. The stale fresh `<clod-note>` provenance note remains harmless context. Conversation-key mode was removed; strict system+tools+model+conversation keying is mandatory, and CLOD_SESSION_KEY is rejected. No protocol workaround or credential change was introduced.

### User-image message shape forces replay

**c2 fixed and live-verified 2026-10-26 (CLI 2.1.308).** Dedicated direct-HTTP runs `scripts/captures/live-verify-20261026-{182054,182706}` plan Fresh → Resume → Resume for a 64×64 red/blue PNG, with identical input/upstream image bytes. The merged upstream user blocks are image → client text (CLI adds newline) → CLI image-source text. The next turn also resumes. Ignored `live_image_resume` passed; an earlier assertion-only failure and its three successful generations are retained in run 182611. Run 182439 additionally resumed a tool-result image successfully. No production change was needed. For the original image64 measurement, Polytoken sends `[…, user [image], user text]`. Previously the image was an extra message in the hashed prefix, producing **PrefixMismatch under the same key**. Resume now hashes everything before the consecutive trailing user messages, validates tool results across them, and merges their ordered blocks into one CLI user record. A sanitized real image64 incoming-body regression plans request 2 as Resume. The recorded cost difference (888 created versus~375 for an ordinary stable request) predates the fix; no new cache usage is claimed. System and tools were unchanged: this was a clod limitation, not a Polytoken break.

### CLI 1×1 image 400, strip-and-retry, and temp-path text

The cause **is recorded in the retry body**. In `remaining-image`, upstream request3 contains the1×1 PNG. Request4,229ms later, replaces message0/block6 with CLI text `[Image removed: the API could not process this image (400 … "Could not process image" …)]`. The API rejected the image with400; the CLI stripped it and silently retried. Request5 has neither image nor removal placeholder. Polytoken and clod passed the original image byte-equal. **31,230 read /1,022 created /2 uncached belongs to request4 alone**; there is no message_start for rejected request3. The summary records attempt3 as rejected and usage_body_index4.

Next to user images the CLI also adds text `[Image: source: /private/tmp/claude-501/…]`. This temp-path block is CLI-originated; it must not be attributed to Polytoken. The64×64 checkerboard avoids the1×1 rejection and succeeds on its first attempt. The earlier422 was a separate harness setup issue. The repaired local-mock setup enables vision and sends `/prompt` images as `{media_type,data:<base64>}`; it was checked before live use.

The proxy now records failed-response status, conservative non-secret headers and decoded error bodies, with compressed-error and passthrough tests. That addition does not erase the retry-placeholder evidence already present in the older body recording.

### Model calls client tools by bare names

Counting rule: **26 finalized assistant tool_use calls across114 stdout captures;1 bare call**. A27th call, title `_infer_title` in `resume-diagnostic-1`, appears only as a stream event and is not included in finalized-assistant count. Stream/final duplicates are not counted twice; earlier runs without stdout are outside the audit.

The bare Opus `tool_search` is neither `mcp__oc__tool_search` nor built-in `ToolSearch`. Evidence: `operator-approved-matrix-claude-opus-5-5-conversation-reveal/stdout-bbf4343237394810bfa8e0caa609e02b.ndjson`; sanitized `fixtures/live/native-search.ndjson`13–24. No PreToolUse hook fired. Dedicated transcript records `No such tool available: tool_search`; result is error_max_turns, num_turns2, terminal_reason max_turns. Polytoken happened to execute its matching client tool, but clod lacked an eligible prefixed-tool checkpoint UUID and replayed. The fallback is conservative; the bare call is an interoperability failure.

**c4: bare-only checkpoint eligibility (offline follow-up using live evidence).** Bare names matching declared client tools are forwarded unchanged without waiting for a nonexistent hook. All-bare turns may now Commit under the same final-assistant, exact streamed/recorded IDs, no-trailing-content and one-message gates as prefixed turns. Mixed prefixed/bare turns still Discard with `MixedBareAndPrefixedToolCalls` and replay. Follow-up live evidence (2026-10-26) establishes a clean **single-call** fork: earlier transcript records 18(tool)→19(error) and new session records 15(tool)→16(error) put the error below, not above, the tool UUID. Run 182301 body 1 contains bare lookup plus exactly one real `REAL_BARE_VALUE` result, no saved error/duplicate/orphan, and succeeds. However the attempted mixed bare/prefixed prompt generated only prefixed calls, and another bare phrasing refused. Mixed-turn safety was not proved; only mixed-turn Discard is retained rather than generalizing the bare-only evidence to that case. Sanitized live `bare-lookup.ndjson` and existing `native-search.ndjson` regression-test Commit with pending IDs and echoed follow-up Resume; neither violates the other gates. No new live call was made for this code change. Unknown bare names remain noncheckpointed. Likely bare-system-name influence is **inferred**, not established.

### Empty-delta checkpoint fix and fresh replay

A separate Haiku call emitted empty partial_json with final input{}. Clod previously parsed the empty string and discarded with InvalidToolInput. It now retains the start input. `checkpoint_edges.rs` replays captured stdout, asserts the tool input is{}, verifies the committed hash matches that echo, and requires the follow-up tool-result request to plan Resume. Native bare-search now commits under the bare-only rules above and its echoed follow-up plans Resume.

Every Fresh request renders history into one user replay message. This changes history structure and loses native history caching even when a previously cached same-session tools/system prefix is reusable. Do not describe retained static-prefix reads as preserved conversation history.

### Thinking/signatures follow-up: live-verified, not a cache-break matrix

On 2026-10-26 with CLI 2.1.308, Sonnet 5.5 produced visible signed thinking in streaming and non-streaming modes. Enabled budget maps to the existing max-thinking flag; adaptive omits that flag; both upstream captures use `thinking:{type:adaptive,display:summarized}`. This verifies compatibility, not budget enforcement. Public CLI help still exposes no thinking flags. No mapping change was needed.

Run 182619 verifies 431-character thinking/signature1712 in non-stream mode, then 380/signature1716 before a streamed tool call. All45 thinking/signature deltas match CLI→client exactly. Echoing signed blocks plans Resume; both resumed bodies preserve signatures exactly; the real tool-result continuation succeeds without invalid-signature errors. Source transcript records19(thinking)→20(tool)→21(denial); fork records15(thinking)→16(tool)→17(real result)→23(answer). `live_thinking_resume` passed. This is not a measured thinking on/off cache-loss experiment.

**Separate follow-up ledger:** 25 upstream requests (Haiku15/Sonnet10), zero captured API errors, under the approved30 ceiling. Raw runs `scripts/captures/live-verify-20261026-{182054,182301,182353,182439,182611,182619,182706}` are outside the original139-request study and generated tables. The live investigation made no production core change; the subsequent operator-approved offline follow-up enables bare-only checkpoints as above. No dependency or credential/config change, deletion or commit. The offline fake-CLI writer now atomically publishes complete log snapshots; bounded EOF observation waits fix related shell races. After fixing two additional timing/observation races exposed during stress testing, the 16-test shell suite passed20 consecutive iterations (320 executions). No new live requests were made. Ignored tests require both consent markers and cap each run at four upstream attempts. Full evidence, limitations, corrected image-test assertion failure, and all seven cleanup folders are in [the dated protocol section](protocol.md#live-verification-2026-10-26). Nothing was deleted; raw recordings may contain private account/prompt data.

## 7. (d) Not measured, with reasons

- **Thinking/effort changes and on/off:** same-model `/model` returned409no_change after two warmups. One offline `/model/options` fix attempt rejected400unknown_option_key reasoning_effort; no paid retry.
- **Daemon restart/PID same session:** offline terminate/continue failed stale live-session health verification. No session-metadata deletion/edit was used to force it.
- **Active unpaused versus paused/completed goal states independently:** automatic goal-driver turns were not safely bounded; measured insertion combines set and pause. Goal clear is measured separately.
- **Controlled title/subagent calls:** not reliably triggered within remaining prepared scenarios; earlier title stream-only event is observed but not a controlled cache triplet.
- **Tool-result image, reminder injection isolation and compaction:** no cheap, safely verified trigger prepared. Do not generalize user-image results to tool-result images.
- **Date rollover:** not measured; no safe supported clock override established.
- **Exact token predictions:** no per-block tokenizer/checkpoint evidence; unknown values stay null rather than byte/4 estimates. All origin labels distinguish exact body differences from heuristic transformed-history attribution.

No more live calls are planned. The study used bounded ceilings and seeded order for ready scenarios, but does not claim complete coverage of every original request.

## 8. Cleanup folders and verification

Canonical dedicated `<HOME>/.claude/projects/-private-…` paths are in every generated run's `cleanup` field. Temporary isolated workdirs were removed by the harness; dedicated CLI transcripts remain for operator cleanup. No deletion performed. Raw stdout may contain account initialization data and is not publishable; exported summaries allowlist numeric usage, hashes, names, indexes, markers/reasons and redacted-home cleanup paths. Offline synthetic rows are labeled.

Final verification: cargo fmt; cargo test --workspace; cargo clippy --workspace --all-targets -- -D warnings; Node gzip/chunked/ceiling and failed-response tests; Python harness/recorder tests; regenerated summaries/table; targeted export sensitive-string scan. No commit.

## 9. Short history appendix

The initial HOME restriction was clarified to permit dedicated CLI transcripts. An early recorder failed to capture usage for18requests, noted separately. Node passthrough plus authoritative CLI usage replaced it. A provider caller-metadata history mismatch was fixed with real fixtures, then generalized to a history-only echoed-field allowlist. The core strict/conversation batch revealed ignored resumed system text; unsafe conversation mode was removed. Empty-delta checkpoint handling was fixed; declared bare-name tool handling was subsequently fixed offline, without live verification. A failed clear setup became a labeled insertion replicate; the repaired clear run measured recovery. The1×1 rejection/CLI retry and64×64 first-attempt success are now correctly separated.
