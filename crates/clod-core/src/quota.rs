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
/// No clock or I/O: callers supply monotonic milliseconds and Unix seconds.
/// Restarting loses the baseline. Missing data is not a zero observation.
#[derive(Default)]
pub struct WeeklyBreaker {
    samples: VecDeque<Sample>,
    reset: Option<u64>,
    last_observed: Option<u64>,
}
impl WeeklyBreaker {
    pub fn observe(&mut self, observation: &Observation, now: u64, epoch: u64) {
        if self.last_observed.is_some_and(|last| now < last) {
            return;
        }
        self.last_observed = Some(now);
        self.expire(now, epoch);
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
    /// Retry delay in milliseconds, or admission. Expiry is checked even when
    /// no CLI is running and no more observations arrive.
    pub fn retry_after(&mut self, now: u64, epoch: u64) -> Option<u64> {
        self.expire(now, epoch);
        let first = self.samples.front()?;
        let latest = self.samples.back()?;
        // Compare against the rounded endpoint, not a subtraction that can
        // round an exact decimal 1pp rise upward. No extra quota margin.
        if latest.usage > first.usage + 0.01 {
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
        for baseline in [0.0, 0.2, 0.67, 0.9] {
            let mut b = WeeklyBreaker::default();
            observe(&mut b, 0, baseline, 10000);
            observe(&mut b, 100, baseline + 0.01, 10000);
            assert_eq!(b.retry_after(100, 1), None);
            observe(&mut b, 200, baseline + 0.010001, 10000);
            assert_eq!(b.retry_after(200, 1), Some(HOUR_MS - 200));
            assert_eq!(b.retry_after(HOUR_MS + 100, 1), None);
        }
    }
    #[test]
    fn repeated_observations_preserve_rolling_baselines() {
        let mut b = WeeklyBreaker::default();
        observe(&mut b, 0, 0.2, 10000);
        observe(&mut b, 100, 0.2, 10000);
        observe(&mut b, 200, 0.2, 10000);
        observe(&mut b, 300, 0.22, 10000);
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
        observe(&mut b, 104, 0.22, 10000);
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
