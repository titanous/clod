# Conservative instruction-name alignment

`CLOD_ALIGN_TOOL_NAMES=1` enables an experimental, default-off transformation. Use a new process and empty session store for any separately authorized experiment. This guide does not authorize deployment, restart, live requests, or changing settings mid-session.

## Transport boundary and grammar

The pure `clod-core` registry rewrites only collected name spans in complete recognized incoming system sections, before clod appends its own notes. Fresh and Resume use the same transformation. String systems and individual text blocks are supported; blocks are cloned, including `cache_control`, and canonical requests are never modified. The existing CLI system-prompt flattening behavior is unchanged. Sections split across blocks are not recognized.

Recognition uses a fixed grammar and exact instruction templates in clod. It does not attempt to recognize arbitrary prose or infer a section's origin.

- Tool Library requires clod's exact recognized introduction, exact backtick headings, unique registry names, matching whitespace-normalized descriptions (or the recognized empty-description fallback), and optional one-line JSON-object examples inside the exact `Example input` fence grammar. Examples are parsed to verify syntax, never rewritten.
- Available/Deferred Tools require exact list prefixes and full registry descriptions. Deferred additionally requires the exact recognized footer.
- Section endings must be end-of-text, a next level-two heading, or one of clod's exact recognized boundary texts: an image-capability paragraph or a general-purpose subagent introduction.
- Duplicate target section headings, repeated entries, unknown names, mismatched descriptions, malformed examples, unsupported format drift, and structural headings embedded in descriptions reject the affected section. Target headings embedded in any registry description reject all instruction rewriting. Fenced sections are ignored, including tilde fences and longer fence delimiters. This is intentionally not a general Markdown parser.
- The callable-phrase allowlist includes the three `job_block`, `job_status`, `job_result` spans in the fully validated library introduction, and `tool_search` in the validated deferred footer. Four additional exact complete sections (targeted reading, writing file content, readiness waiting, and web search) select nine backtick callable slots. These require unique headings outside fences, exact full template text, and end-of-text or newline-only separation before another level-two/three heading. Missing registry entries remain literal. No free-standing phrase search is performed.

All bytes outside accepted spans are copied exactly. Names are exact registry client-to-wire mappings applied once, never suffix aliases. A client/wire collision disables instruction rewriting entirely rather than choosing a non-idempotent interpretation. The existing structured replay mapping remains one-pass on canonical calls. Dynamic ASCII registration names (including already-prefixed names) work; empty/non-ASCII names conservatively remain unchanged.

This recognizes structure, not authenticity: a caller can provide a byte-perfect copy of a recognized section. Plain system text has no provenance marker that could distinguish such a copy. In that case only the same validated name slots change. Ambiguous heading/description structures are rejected rather than treated as recognized boundaries.

## Intentionally uncovered

Arbitrary inline references, prose outside validated slots, description contents, arbitrary code, tool arguments/results, schema enum/default/const/examples, and `list_jobs.show_type` literals are never recursively rewritten. Tool schemas/descriptions themselves are not mutated. The exact-pair mapping note remains the fallback for these references and for rejected sections. Canonical requests, session keys, prefix hashes, output prefix stripping, and checkpoint guards are unchanged. No argument-type repair is attempted.

## Regression coverage and limitations

Pure Rust regressions cover UTF-8 byte preservation, overlapping names, dynamic/already-prefixed/empty/Unicode names, collisions, repeat rendering, fake/duplicate headings, format drift, fenced code and JSON examples, deferred callable slots, both spawn plans, canonical request serialization including cache-control, and session key/prefix-hash invariance. The sanitized production 16-call replay regression covers structured replay.

These checks exercise clod's transformation rules, not model efficacy. Matching a template does not establish its origin, and passing offline checks does not establish that live models will select the intended tool names.
