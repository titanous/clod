# Subscription usage observations

clod recognizes Claude CLI `rate_limit_event` records. It logs validated `five_hour`
and `seven_day` utilization as fractions and percentages, reset Unix seconds,
observation Unix seconds and process-monotonic milliseconds, and source
`cli_rate_limit_event`. With `CLOD_MEASURE_LOG`, these are separate `kind: quota`
records. Only window names, source and numeric values are recorded: no prompts,
account initialization, session identifiers or overage metadata. Logging works
without enabling the breaker. Missing windows are not logged as zero. Unknown
fields remain tolerated; an explicitly named representative window is used only
for that window when a valid unified value is unavailable.

## Optional rolling-hour guard

The guard is disabled by default. Set **`CLOD_WEEKLY_BREAKER=1`** on the next
operator-managed deployment to enable it; every other set value (including `0`
and empty) fails startup. No account settings are changed.

`CLOD_WEEKLY_BREAKER_PERCENT` sets the threshold in **percentage points**, not a
percentage of the baseline. When unset it defaults to **1** (0.01 utilization).
For a two-point threshold, set both `CLOD_WEEKLY_BREAKER=1` and
`CLOD_WEEKLY_BREAKER_PERCENT=2`. Fractional values such as `0.5` are supported.
The value must be a finite number greater than 0 and at most 100; empty,
non-numeric, NaN, infinite and out-of-range values fail startup even if the guard
is disabled. Setting the threshold alone does not enable the guard.

The guard compares the latest valid weekly utilization with the minimum retained
baseline from the same weekly reset epoch within the previous 3,600 seconds.
It rejects new requests only for a rise **strictly greater than the configured
threshold**. The floating-point check compares the latest value with
`baseline + threshold_fraction`, avoiding subtraction noise at an exact threshold
rise. A downward correction starts a new baseline. A changed
or elapsed reset invalidates the old baseline. Invalid, missing, out-of-order or
out-of-range observations cannot manufacture a rise. Missing data does not erase
valid recent evidence, but evidence expires even when no new events arrive.
Observations exactly 3,600 seconds old are expired.

The guard is shared across sessions in one process and checked after session and
concurrency queue waits, before exchange planning or spawning. Rejections are local
HTTP 429 `rate_limit_error` responses with a retry delay; checkpoints are untouched.
In-flight work continues, `/health` works, and the guard automatically admits again
when the rolling condition expires or is invalidated. It never launches a probe
or half-open request automatically. The delay is when the current earliest
baseline expires, not a guarantee about future observations.

This is an observation-based safety guard, not quota enforcement or per-request
billing. CLI events can reflect usage by other clients. No initial zero is assumed;
a process restart loses all history and needs at least two valid observations.
Nothing is persisted. Extremely dense observations are capped at 4,096
samples: overflow starts a new baseline and fails open. Repeated observations
retain their timestamps so baseline expiry remains accurate below this cap. All decisions are in the pure core; the
shell supplies clocks and performs logging and HTTP I/O.

Deployment: review logs offline, configure the opt-in in the deployment environment,
and restart only with operator approval. Unset it and deploy/restart to disable.
No restart, live CLI invocation, or live quota validation is part of offline tests.
