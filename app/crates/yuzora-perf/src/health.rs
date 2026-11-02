//! How sampling went: the last [`WINDOW`] polls' outcomes, the last error, and
//! what one sample cost. Ported from beta.1's perfStore.
//!
//! A fixed rolling window, so the state stays bounded and goes stale: once
//! sampling works again, the warning leaves within one window. A warning that
//! never expired would forever cover what's true now.

use serde::Serialize;
use std::collections::VecDeque;
use std::time::Duration;

/// Polls kept (one minute at one poll every 2 s).
pub const WINDOW: usize = 30;

/// One poll's outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Ok,
    /// Sampling failed (the error is kept).
    Failed,
    /// Sampling answered without the app's own process: no data, which is
    /// not zero. The last good sample stays on screen, with a warning.
    Empty,
    /// The window wasn't in front: not sampled.
    SkippedNoFocus,
}

/// The window's makeup now (not a count since some earlier read).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Summary {
    pub attempts: u32,
    pub failures: u32,
    pub empty: u32,
    pub skipped: u32,
    /// The last failure's error while one is in the window.
    pub last_error: Option<String>,
}

/// What one sample took, over the last [`WINDOW`] samples.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Cost {
    pub last_ms: f64,
    pub median_ms: f64,
    pub p95_ms: f64,
    pub samples: u32,
}

#[derive(Clone, Debug, Default)]
pub struct Health {
    outcomes: VecDeque<Outcome>,
    last_error: Option<String>,
    costs: VecDeque<Duration>,
}

impl Health {
    pub fn record(&mut self, outcome: Outcome, error: Option<String>) {
        self.outcomes.push_back(outcome);
        while self.outcomes.len() > WINDOW {
            self.outcomes.pop_front();
        }
        // The error follows the window: gone once no failure is left in it.
        self.last_error = if !self.outcomes.contains(&Outcome::Failed) {
            None
        } else if outcome == Outcome::Failed {
            Some(error.unwrap_or_else(|| "unknown error".into()))
        } else {
            self.last_error.take()
        };
    }

    pub fn summary(&self) -> Summary {
        let count = |kind: Outcome| self.outcomes.iter().filter(|o| **o == kind).count() as u32;
        Summary {
            attempts: self.outcomes.len() as u32,
            failures: count(Outcome::Failed),
            empty: count(Outcome::Empty),
            skipped: count(Outcome::SkippedNoFocus),
            last_error: self.last_error.clone(),
        }
    }

    /// A sample's duration (skipped polls cost nothing and aren't recorded).
    pub fn record_cost(&mut self, took: Duration) {
        self.costs.push_back(took);
        while self.costs.len() > WINDOW {
            self.costs.pop_front();
        }
    }

    pub fn cost(&self) -> Option<Cost> {
        let last = *self.costs.back()?;
        let ms: Vec<f64> = self
            .costs
            .iter()
            .map(|d| d.as_secs_f64() * 1000.0)
            .collect();
        Some(Cost {
            last_ms: last.as_secs_f64() * 1000.0,
            median_ms: median(&ms),
            p95_ms: percentile(&ms, 95.0),
            samples: ms.len() as u32,
        })
    }
}

fn sorted(values: &[f64]) -> Vec<f64> {
    let mut v = values.to_vec();
    v.sort_by(f64::total_cmp);
    v
}

/// The middle value (the mean of the two middle ones for an even count).
pub fn median(values: &[f64]) -> f64 {
    let v = sorted(values);
    match v.len() {
        0 => 0.0,
        n if n % 2 == 1 => v[n / 2],
        n => (v[n / 2 - 1] + v[n / 2]) / 2.0,
    }
}

/// Nearest rank: the smallest value at least `p` percent of the values are at or below.
pub fn percentile(values: &[f64], p: f64) -> f64 {
    let v = sorted(values);
    if v.is_empty() {
        return 0.0;
    }
    let rank = ((p / 100.0) * v.len() as f64).ceil().max(1.0) as usize;
    v[rank.min(v.len()) - 1]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_is_bounded_and_counts_each_outcome() {
        let mut h = Health::default();
        for _ in 0..40 {
            h.record(Outcome::Ok, None);
        }
        h.record(Outcome::Empty, None);
        h.record(Outcome::SkippedNoFocus, None);
        h.record(Outcome::Failed, Some("denied".into()));
        let s = h.summary();
        assert_eq!(
            (s.attempts, s.failures, s.empty, s.skipped),
            (WINDOW as u32, 1, 1, 1)
        );
        assert_eq!(s.last_error.as_deref(), Some("denied"));
    }

    #[test]
    fn the_last_error_expires_with_its_failures() {
        let mut h = Health::default();
        h.record(Outcome::Failed, Some("first".into()));
        h.record(Outcome::Ok, None);
        assert_eq!(
            h.summary().last_error.as_deref(),
            Some("first"),
            "kept while a failure is in the window"
        );
        h.record(Outcome::Failed, None);
        assert_eq!(h.summary().last_error.as_deref(), Some("unknown error"));
        for _ in 0..WINDOW {
            h.record(Outcome::Ok, None);
        }
        let s = h.summary();
        assert_eq!(
            (s.failures, s.last_error),
            (0, None),
            "a recovered sampler shows no warning"
        );
    }

    #[test]
    fn skipped_and_empty_are_not_failures() {
        let mut h = Health::default();
        h.record(Outcome::SkippedNoFocus, None);
        h.record(Outcome::Empty, None);
        let s = h.summary();
        assert_eq!(
            (s.failures, s.empty, s.skipped, s.last_error),
            (0, 1, 1, None)
        );
    }

    #[test]
    fn cost_reports_median_and_p95_of_recent_samples() {
        let mut h = Health::default();
        assert!(h.cost().is_none());
        for ms in 1..=20u64 {
            h.record_cost(Duration::from_millis(ms));
        }
        let c = h.cost().unwrap();
        assert_eq!(
            (c.samples, c.last_ms, c.median_ms, c.p95_ms),
            (20, 20.0, 10.5, 19.0)
        );
        for _ in 0..WINDOW {
            h.record_cost(Duration::from_millis(2));
        }
        assert_eq!(h.cost().unwrap().samples, WINDOW as u32);
        assert_eq!(h.cost().unwrap().p95_ms, 2.0);
        assert_eq!(median(&[3.0, 1.0, 2.0]), 2.0);
        assert_eq!(percentile(&[5.0], 95.0), 5.0);
    }
}
