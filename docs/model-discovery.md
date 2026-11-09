# Model discovery validation

Validated on 2026-11-09 with Claude CLI **2.1.319** and Polytoken **0.9.7**.
No user message or generation request was sent. The production service was not
restarted; validation used a separate clod listener on loopback.

## Claude CLI

`GET /v1/models` initialized the real CLI and returned 14 selections, including
`default`, `opus`, `fable`, `sonnet`, `haiku`, and explicit versioned model IDs.
These are the CLI's selectable IDs, not a guarantee of subscription availability.
A separate initialize-only probe inspected only the `models` member; account
initialization data was neither printed nor exported.

The observed model metadata included `value`, `resolvedModel`, `displayName`,
`description`, `supportsEffort`, `supportedEffortLevels`,
`supportsAdaptiveThinking`, `supportsThinkingOff`, `supportsAutoMode`, and
`supportsFastMode`. Optional fields varied by entry. clod maps `value` to `id`,
`displayName` to `display_name`, and supplies `type: model`. It also exposes
`description`, `resolved_model`, `supports_effort`, `supported_effort_levels`,
`supports_adaptive_thinking`, `supports_thinking_off`, `supports_auto_mode`, and
`supports_fast_mode` when reported. Missing fields stay omitted, and explicit
false values are preserved. Only these typed model metadata fields are exported;
the rest of the initialization response is never included. The CLI catalog did
not provide input context limits, output-token limits, vision support, or
creation timestamps.

The enriched response was subsequently checked against the real CLI: lookup
of `opus` returned its resolved ID, description, five effort levels, adaptive
thinking and mode flags, including `supports_thinking_off: false`. Polytoken
0.9.7's Models → Add Custom picker still displayed all 14 IDs with this response.

## Polytoken configuration setup

Validation ran the installed `polytoken config ui` in a pseudo-terminal with
isolated HOME, XDG configuration/data/cache/state, and working directories.
Only the real Claude child used the normal subscription authentication.

1. Configured a custom Anthropic-compatible provider with the loopback base URL
   (without `/v1/messages`) and a dedicated test bearer key.
2. Opened Models → Add Custom. The UI fetched and displayed all 14 CLI IDs.
3. Selected `opus`. The resulting model name was `clod-validation/opus` and its
   wire model ID was `opus`.
4. Confirmed context window was blank and reasoning was the generic `thinking`
   default, rather than the CLI's advertised effort configuration.
5. Confirmed saving without context window reported the required-positive-context
   validation error.
6. Entered an explicit context window of 1,000,000 as a **test configuration
   value**, not a discovered or validated capability. Saved successfully.
7. Ran `polytoken config validate`: `merged config valid`.
8. Ran `polytoken print models --format json`: the authored model was present,
   with wire ID `opus` and the generic thinking configuration.

The generic setup discovery flow consumes model IDs; it does not auto-populate
context limits, output limits, vision, model variant, or effort controls from
this endpoint. These settings still require explicit configuration. The README
example demonstrates authored settings, but they must be checked against the
selected model and account. Merely adding more fields to clod's model-list
response cannot make this setup flow consume them.

This validates discovery and configuration persistence, **not generation,
reasoning behavior, context capacity, or model availability**. No production
configuration or sibling repository was modified.

Temporary validation workdir: `/tmp/clod-model-validation-0em67p/work`.
Any Claude transcript directory corresponding to that workdir is left for
operator cleanup; the validation does not delete CLI transcripts.
