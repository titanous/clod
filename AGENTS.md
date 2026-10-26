# Working on clod

- `meridian/` is a read-only reference checkout: never modify, build or import it. Do not modify the sibling Polytoken repository as part of clod work.
- All protocol, session, retry, timeout and checkpoint decisions belong in `crates/clod-core`, a pure `#![no_std]` + alloc crate. The tokio/axum shell in `crates/clod` only performs actions and reports observations. Preserve this boundary.
- The core dependency allowlist is enforced by `scripts/check-core-deps` through cargo tests, including a negative metadata fixture. Do not casually add dependencies or enable std features. A deliberate dependency change needs review of the full normal transitive graph, updating the script's allowlist and negative test as appropriate, then running the guard and workspace tests. Direct dependencies remain serde, serde_json, sha2 and thiserror with the reviewed feature settings.
- Before a Claude CLI upgrade, obtain operator approval and re-run Phase 0 captures (`scripts/capture*.sh` / `scripts/capture*.mjs`), review injected prompts and protocol differences, update sanitized fixtures and `docs/protocol.md`, then run the live smoke test. Update `TESTED_CLAUDE_VERSION` only with evidence.

## Offline checks

From the repository root:

```sh
cargo fmt --all
cargo test --workspace
cargo test -p clod --features test-support
cargo clippy --workspace --all-targets -- -D warnings
python3 -I scripts/polytoken-harness/test_harness.py
python3 -I scripts/test-cache-recorder.py
node scripts/test-upstream.mjs
node scripts/test-upstream-error.mjs
```

The test-support feature enables the fake-claude binary and scripted shell tests. The live smoke is `#[ignore]`: setting CLOD_LIVE alone does not run it. Never run it during routine offline checks.

## Authorized live smoke (AC.12)

Only after explicit operator approval, with the real CLI authenticated:

```sh
CLOD_LIVE=1 cargo test -p clod --test live_smoke -- --ignored --exact live_smoke --nocapture
```

Without `CLOD_LIVE=1` the test refuses before spawning anything. It makes five Haiku requests: text, text resume, three parallel tools, all results followed by a second tool round, then final results. It checks resume spawn plans, resumed cache reads, missing-message errors and values from both rounds. It starts clod on loopback with a dedicated temporary workdir and measurement log. It does not delete real CLI transcripts; the printed workdir identifies the dedicated ~/.claude/projects folder for operator cleanup. The smoke is model-dependent and not a substitute for reviewing transcript ordering of bare tool calls. No bare-name checkpoint is enabled by this test.

## Measurements and fixtures

See `docs/polytoken-harness.md`, `docs/cache-measurements.md` and scripts under `scripts/`. The default Polytoken harness uses an isolated HOME/XDG/config/workdir and a local scripted provider. Measurement wrappers record incoming, CLI and upstream evidence; raw account initialization, system prompts, paths and headers can contain private data. Sanitize before exporting fixtures and retain provenance/limitations; do not infer cache tokens from bytes or sum duplicate usage records.

Live calls require explicit operator approval and bounded request ceilings. In measurement runs, real HOME is permitted **only for the claude child** to use subscription authentication; Polytoken remains isolated. Never set CLAUDE_CONFIG_DIR or edit credentials/settings. Read only dedicated measurement transcripts, report cleanup folders, and leave deletion to the operator. Offline fixture replay is not live validation.
