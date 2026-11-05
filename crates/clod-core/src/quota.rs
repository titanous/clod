//! Account-wide subscription observations and a process-local rolling-hour gate.
use alloc::collections::VecDeque;
use serde::Serialize;
use serde_json::Value;

pub const HOUR_MS: u64 = 3_600_000;
const MAX_SAMPLES: usize = 4096;

#[derive(Clone, Copy, Debug, Serialize, PartialEq)]
pub struct Window {
    pub utilization: f64,
    pub resets_at: u64,
}
impl Window {
    fn parse(value: &Value) -> Option<Self> {
        let window = Self {
            utilization: value.get("utilization")?.as_f64()?,
            resets_at: value.get("resetsAt")?.as_u64()?,
        };
        window.valid().then_some(window)
    }
    pub fn valid(self) -> bool {
        self.utilization.is_finite()
            && (0.0..=1.0).contains(&self.utilization)
            && self.resets_at > 0
    }
}
#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct Observation {
    pub five_hour: Option<Window>,
    pub seven_day: Option<Window>,
    pub status: Option<alloc::string::String>,
}
impl Observation {
    /// Unknown fields and malformed optional fields never invalidate the stream.
    pub fn parse(info: &Value) -> Self {
        let windows = &info["unifiedWindows"];
        let representative = info["rateLimitType"].as_str();
        Self {
            five_hour: Window::parse(&windows["five_hour"]).or_else(|| {
                (representative == Some("five_hour"))
                    .then(|| Window::parse(info))
                    .flatten()
            }),
            seven_day: Window::parse(&windows["seven_day"]).or_else(|| {
                (representative == Some("seven_day"))
                    .then(|| Window::parse(info))
                    .flatten()
            }),
            status: info["status"].as_str().map(Into::into),
        }
    }
}
#[derive(Clone, Copy, Debug)]
struct Sample {
    at: u64,
    usage: f64,
}
/// A validated rise in utilization, expressed as a fraction (0.01 is 1pp).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WeeklyThreshold(f64);
#[derive(Debug, thiserror::Error)]
#[error("weekly breaker threshold must be finite, greater than zero and at most 1 utilization")]
pub struct InvalidWeeklyThreshold;
impl WeeklyThreshold {
    pub fn new(fraction: f64) -> Result<Self, InvalidWeeklyThreshold> {
        if !fraction.is_finite() || fraction <= 0.0 || fraction > 1.0 {
            return Err(InvalidWeeklyThreshold);
        }
        Ok(Self(fraction))
    }
    pub fn percent(self) -> f64 {
        self.0 * 100.0
    }
}
impl Default for WeeklyThreshold {
    fn default() -> Self {
        Self(0.01)
    }
}
/// Diagnostic state at a caller-supplied instant. Reset epochs are quota resets,
/// not promises of admission. Delays assume no further observations.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Snapshot {
    pub enabled: bool,
    pub state: &'static str,
    pub threshold_utilization: f64,
    pub window_ms: u64,
    pub baseline_utilization: Option<f64>,
    pub current_utilization: Option<f64>,
    pub delta_utilization: Option<f64>,
    pub weekly_observation_age_ms: Option<u64>,
    pub weekly_resets_at: Option<u64>,
    pub weekly_reset_expired: Option<bool>,
    pub five_hour_utilization: Option<f64>,
    pub five_hour_resets_at: Option<u64>,
    pub five_hour_observation_age_ms: Option<u64>,
    pub five_hour_reset_expired: Option<bool>,
    pub earliest_recheck_ms: Option<u64>,
    pub predicted_reopen_ms: Option<u64>,
}

/// No clock or I/O: callers supply monotonic milliseconds and Unix seconds.
/// Restarting loses the baseline. Missing data is not a zero observation.
#[derive(Default)]
pub struct WeeklyBreaker {
    threshold: WeeklyThreshold,
    samples: VecDeque<Sample>,
    reset: Option<u64>,
    last_observed: Option<u64>,
    weekly: Option<(Window, u64)>,
    five_hour: Option<(Window, u64)>,
}
impl WeeklyBreaker {
    pub fn new(threshold: WeeklyThreshold) -> Self {
        Self {
            threshold,
            ..Self::default()
        }
    }
    pub fn observe(&mut self, observation: &Observation, now: u64, epoch: u64) {
        if self.last_observed.is_some_and(|last| now < last) {
            return;
        }
        self.last_observed = Some(now);
        self.expire(now, epoch);
        if let Some(window) = observation.five_hour.filter(|w| {
            w.valid()
                && (w.resets_at > epoch
                    || self.five_hour.is_none_or(|(old, _)| old.resets_at <= epoch))
        }) {
            self.five_hour = Some((window, now));
        }
        if let Some(window) = observation.seven_day.filter(|w| {
            w.valid()
                && (w.resets_at > epoch
                    || self.weekly.is_none_or(|(old, _)| old.resets_at <= epoch))
        }) {
            self.weekly = Some((window, now));
        }
        let Some(window) = observation
            .seven_day
            .filter(|w| w.valid() && w.resets_at > epoch)
        else {
            return;
        };
        // A reset or a downward correction starts a new baseline: neither is
        // evidence of a rise. Never compare observations across reset epochs.
        if self.reset != Some(window.resets_at)
            || self
                .samples
                .back()
                .is_some_and(|s| window.utilization < s.usage)
        {
            self.samples.clear();
        }
        self.reset = Some(window.resets_at);
        if self.samples.len() == MAX_SAMPLES {
            // Bounded storage under arbitrarily dense events. Losing a baseline
            // must fail open rather than manufacture a rise.
            self.samples.clear();
        }
        self.samples.push_back(Sample {
            at: now,
            usage: window.utilization,
        });
    }
    fn expire(&mut self, now: u64, epoch: u64) {
        while self
            .samples
            .front()
            .is_some_and(|s| now.saturating_sub(s.at) >= HOUR_MS)
        {
            self.samples.pop_front();
        }
        if self.reset.is_some_and(|reset| epoch >= reset) {
            self.samples.clear();
            self.reset = None;
        }
    }
    pub fn snapshot(&mut self, enabled: bool, now: u64, epoch: u64) -> Snapshot {
        let recheck = self.retry_after(now, epoch);
        let first = self.samples.front();
        let latest = self.samples.back();
        // All samples preceding the first non-triggering baseline must expire.
        // Using only the oldest sample would understate a repeated plateau.
        let reopen = recheck.and_then(|_| {
            let latest = latest?;
            self.samples
                .iter()
                .take_while(|s| latest.usage > s.usage + self.threshold.0)
                .last()
                .map(|s| {
                    let delay = HOUR_MS.saturating_sub(now.saturating_sub(s.at));
                    delay.min(
                        self.reset
                            .unwrap_or(epoch)
                            .saturating_sub(epoch)
                            .saturating_mul(1000),
                    )
                })
        });
        Snapshot {
            enabled,
            state: if !enabled {
                "disabled"
            } else if recheck.is_some() {
                "open"
            } else if self.samples.len() < 2 {
                "unknown"
            } else {
                "closed"
            },
            threshold_utilization: self.threshold.0,
            window_ms: HOUR_MS,
            baseline_utilization: first.map(|s| s.usage),
            current_utilization: self.weekly.map(|(w, _)| w.utilization),
            delta_utilization: first.zip(latest).map(|(a, b)| b.usage - a.usage),
            weekly_observation_age_ms: self.weekly.map(|(_, at)| now.saturating_sub(at)),
            weekly_resets_at: self.weekly.map(|(w, _)| w.resets_at),
            weekly_reset_expired: self.weekly.map(|(w, _)| w.resets_at <= epoch),
            five_hour_utilization: self.five_hour.map(|(w, _)| w.utilization),
            five_hour_resets_at: self.five_hour.map(|(w, _)| w.resets_at),
            five_hour_observation_age_ms: self.five_hour.map(|(_, at)| now.saturating_sub(at)),
            five_hour_reset_expired: self.five_hour.map(|(w, _)| w.resets_at <= epoch),
            earliest_recheck_ms: recheck,
            predicted_reopen_ms: reopen,
        }
    }
    /// Earliest baseline recheck, not a promise that the breaker will reopen.
    /// Retry delay in milliseconds, or admission. Expiry is checked even when
    /// no CLI is running and no more observations arrive.
    pub fn retry_after(&mut self, now: u64, epoch: u64) -> Option<u64> {
        self.expire(now, epoch);
        let first = self.samples.front()?;
        let latest = self.samples.back()?;
        // Compare against the rounded endpoint, not a subtraction that can
        // round an exact decimal threshold rise upward. No extra quota margin.
        if latest.usage > first.usage + self.threshold.0 {
            Some(HOUR_MS.saturating_sub(now.saturating_sub(first.at)))
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn observe(b: &mut WeeklyBreaker, at: u64, usage: f64, reset: u64) {
        b.observe(
            &Observation {
                seven_day: Some(Window {
                    utilization: usage,
                    resets_at: reset,
                }),
                ..Observation::default()
            },
            at,
            1,
        );
    }
    #[test]
    fn strict_threshold_expiry_and_silence() {
        for threshold in [0.01, 0.02, 0.005] {
            for baseline in [0.0, 0.2, 0.67, 0.9] {
                let mut b = WeeklyBreaker::new(WeeklyThreshold::new(threshold).unwrap());
                observe(&mut b, 0, baseline, 10000);
                observe(&mut b, 100, baseline + threshold, 10000);
                assert_eq!(b.retry_after(100, 1), None);
                observe(&mut b, 200, baseline + threshold + 0.000001, 10000);
                assert_eq!(b.retry_after(200, 1), Some(HOUR_MS - 200));
                assert_eq!(b.retry_after(HOUR_MS + 100, 1), None);
            }
        }
        assert_eq!(WeeklyThreshold::default().percent(), 1.0);
    }
    #[test]
    fn diagnostic_timing_and_unknown_observations() {
        let mut b = WeeklyBreaker::default();
        let empty = b.snapshot(true, 0, 1);
        assert_eq!(empty.state, "unknown");
        assert_eq!(empty.weekly_resets_at, None);
        assert_eq!(empty.five_hour_resets_at, None);
        observe(&mut b, 0, 0.2, 10000);
        observe(&mut b, 100, 0.2, 10000);
        observe(&mut b, 200, 0.2, 10000);
        observe(&mut b, 300, 0.23, 10000);
        let s = b.snapshot(true, 350, 1);
        assert_eq!(s.state, "open");
        assert_eq!(s.earliest_recheck_ms, Some(HOUR_MS - 350));
        assert_eq!(s.predicted_reopen_ms, Some(HOUR_MS - 150));
        assert_eq!(s.weekly_observation_age_ms, Some(50));
        assert_eq!(b.snapshot(false, 350, 1).state, "disabled");
        assert_eq!(b.snapshot(true, HOUR_MS + 200, 1).predicted_reopen_ms, None);
        let expired = b.snapshot(true, HOUR_MS + 200, 10000);
        assert_eq!(expired.state, "unknown");
        assert_eq!(expired.weekly_reset_expired, Some(true));
        assert_eq!(expired.baseline_utilization, None);
        b.observe(
            &Observation {
                five_hour: Some(Window {
                    utilization: 0.9,
                    resets_at: 30,
                }),
                ..Observation::default()
            },
            HOUR_MS + 201,
            20,
        );
        let s = b.snapshot(true, HOUR_MS + 211, 31);
        assert_eq!(s.five_hour_resets_at, Some(30));
        assert_eq!(s.five_hour_reset_expired, Some(true));
        assert_eq!(s.five_hour_observation_age_ms, Some(10));
    }
    #[test]
    fn invalid_thresholds_are_rejected() {
        for fraction in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 0.0, -0.01, 1.01] {
            assert!(WeeklyThreshold::new(fraction).is_err());
        }
        assert_eq!(WeeklyThreshold::new(1.0).unwrap().percent(), 100.0);
    }
    #[test]
    fn repeated_observations_preserve_rolling_baselines() {
        let mut b = WeeklyBreaker::default();
        observe(&mut b, 0, 0.2, 10000);
        observe(&mut b, 100, 0.2, 10000);
        observe(&mut b, 200, 0.2, 10000);
        observe(&mut b, 300, 0.23, 10000);
        assert_eq!(b.retry_after(HOUR_MS, 1), Some(100));
        assert_eq!(b.retry_after(HOUR_MS + 100, 1), Some(100));
        assert_eq!(b.retry_after(HOUR_MS + 200, 1), None);
        assert!(matches!(
            crate::protocol::parse(
                "{\"type\":\"rate_limit_event\",\"rate_limit_info\":{\"future\":true}}"
            ),
            Ok(crate::protocol::Record::Quota(_))
        ));
    }
    #[test]
    fn invalid_missing_out_of_order_reset_and_decrease() {
        let mut b = WeeklyBreaker::default();
        observe(&mut b, 100, 0.2, 10000);
        for usage in [f64::NAN, f64::INFINITY, -1.0, 1.1] {
            observe(&mut b, 101, usage, 10000);
        }
        b.observe(&Observation::default(), 102, 1);
        observe(&mut b, 99, 0.9, 10000);
        assert_eq!(b.retry_after(103, 1), None);
        observe(&mut b, 104, 0.23, 10000);
        assert!(b.retry_after(104, 1).is_some());
        observe(&mut b, 105, 0.21, 10000);
        assert_eq!(b.retry_after(105, 1), None);
        observe(&mut b, 106, 0.8, 20000);
        assert_eq!(b.retry_after(106, 1), None);
        observe(&mut b, 107, 0.9, 20000);
        assert_eq!(b.retry_after(107, 20000), None);
    }
    #[test]
    fn bounded_repeats_overflow_and_clock_extremes() {
        let mut b = WeeklyBreaker::default();
        for at in 0..10000 {
            observe(&mut b, at, 0.2, 10000);
        }
        assert!(b.samples.len() <= MAX_SAMPLES);
        for at in 10000..20000 {
            observe(&mut b, at, (at as f64) / 30000.0, 10000);
        }
        assert!(b.samples.len() <= MAX_SAMPLES);
        assert_eq!(b.retry_after(u64::MAX, 1), None);
    }
    #[test]
    fn representative_is_not_weekly_and_unknowns_are_tolerated() {
        let o = Observation::parse(
            &serde_json::json!({"rateLimitType":"five_hour", "utilization":0.5,"resetsAt":100,"future":{},"unifiedWindows":{"seven_day":{"utilization":"bad"}}}),
        );
        assert!(o.five_hour.is_some());
        assert!(o.seven_day.is_none());
        let o = Observation::parse(
            &serde_json::json!({"rateLimitType":"seven_day", "utilization":0.5,"resetsAt":100}),
        );
        assert!(o.seven_day.is_some());
    }
}
