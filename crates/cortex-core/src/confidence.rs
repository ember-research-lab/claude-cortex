//! Confidence semantics for learnings.
//!
//! Outcomes adjust confidence by a fixed delta (matches v2):
//! - Success: +0.10
//! - Partial: +0.02
//! - Failure: -0.15
//!
//! Confidence is bounded to [0.0, 1.0] and decays with a 180-day exponential
//! half-life: `c(t) = c0 * 0.5 ^ (age_days / 180)`.

use chrono::{DateTime, Utc};

use crate::models::OutcomeResult;

pub const SUCCESS_DELTA: f64 = 0.10;
pub const PARTIAL_DELTA: f64 = 0.02;
pub const FAILURE_DELTA: f64 = -0.15;
pub const HALF_LIFE_DAYS: f64 = 180.0;

pub const OUTCOME_DELTAS: [(OutcomeResult, f64); 3] = [
    (OutcomeResult::Success, SUCCESS_DELTA),
    (OutcomeResult::Partial, PARTIAL_DELTA),
    (OutcomeResult::Failure, FAILURE_DELTA),
];

#[derive(Debug, Clone, Copy)]
pub struct ConfidenceConfig {
    pub success_delta: f64,
    pub partial_delta: f64,
    pub failure_delta: f64,
    pub half_life_days: f64,
}

impl Default for ConfidenceConfig {
    fn default() -> Self {
        Self {
            success_delta: SUCCESS_DELTA,
            partial_delta: PARTIAL_DELTA,
            failure_delta: FAILURE_DELTA,
            half_life_days: HALF_LIFE_DAYS,
        }
    }
}

pub fn delta_for(result: OutcomeResult) -> f64 {
    delta_for_with(result, &ConfidenceConfig::default())
}

pub fn delta_for_with(result: OutcomeResult, cfg: &ConfidenceConfig) -> f64 {
    match result {
        OutcomeResult::Success => cfg.success_delta,
        OutcomeResult::Partial => cfg.partial_delta,
        OutcomeResult::Failure => cfg.failure_delta,
    }
}

pub fn apply_outcome_delta(confidence: f64, result: OutcomeResult) -> f64 {
    apply_outcome_delta_with(confidence, result, &ConfidenceConfig::default())
}

pub fn apply_outcome_delta_with(
    confidence: f64,
    result: OutcomeResult,
    cfg: &ConfidenceConfig,
) -> f64 {
    let delta = delta_for_with(result, cfg);
    (confidence + delta).clamp(0.0, 1.0)
}

pub fn decay_confidence(confidence: f64, last_applied: DateTime<Utc>, now: DateTime<Utc>) -> f64 {
    decay_confidence_with(confidence, last_applied, now, &ConfidenceConfig::default())
}

pub fn decay_confidence_with(
    confidence: f64,
    last_applied: DateTime<Utc>,
    now: DateTime<Utc>,
    cfg: &ConfidenceConfig,
) -> f64 {
    let age_days = (now - last_applied).num_seconds() as f64 / 86_400.0;
    if age_days <= 0.0 {
        return confidence;
    }
    let factor = 0.5_f64.powf(age_days / cfg.half_life_days);
    (confidence * factor).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn outcome_deltas_match_v2() {
        assert_eq!(delta_for(OutcomeResult::Success), 0.10);
        assert_eq!(delta_for(OutcomeResult::Partial), 0.02);
        assert_eq!(delta_for(OutcomeResult::Failure), -0.15);
    }

    #[test]
    fn confidence_clamps_to_bounds() {
        assert_eq!(apply_outcome_delta(0.95, OutcomeResult::Success), 1.0);
        assert_eq!(apply_outcome_delta(0.05, OutcomeResult::Failure), 0.0);
    }

    #[test]
    fn decay_halves_at_180_days() {
        let now = Utc.with_ymd_and_hms(2026, 5, 6, 0, 0, 0).unwrap();
        let then = now - chrono::Duration::days(180);
        let decayed = decay_confidence(0.6, then, now);
        assert!((decayed - 0.3).abs() < 1e-9, "got {decayed}");
    }

    #[test]
    fn future_last_applied_is_no_op() {
        let now = Utc.with_ymd_and_hms(2026, 5, 6, 0, 0, 0).unwrap();
        let later = now + chrono::Duration::days(1);
        assert_eq!(decay_confidence(0.5, later, now), 0.5);
    }
}
