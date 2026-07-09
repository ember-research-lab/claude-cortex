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

use crate::models::{InferredTier, Origin, OutcomeResult};

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

// ===========================================================================
// v-next EPISTEMIC confidence model (docs/vnext-substrate-spec.md §3).
//
// Added ALONGSIDE the flat model above — the flat `apply_outcome_delta` /
// `decay_confidence` are still used by the SMB ConfidenceOracle (cortex-confidence
// crate) and must not change. These epistemic functions drive the cortex LEDGER
// path only. They live entirely on the mutable `Reinforcement` side-file and are
// never hashed into a block (see hashing::canonical_learning_value).
//
// Parameters are eyeball defaults pending calibration (§9/§10); this code gates
// the MECHANICS (climb / relax-to-prior / bounds / reclassification), not the
// calibrated values.
// ===========================================================================

/// Per-event corroboration learning rate (weaker than an applied success).
pub const CORROBORATION_LR: f64 = 0.05;
/// Effective-confidence threshold for promotion to `Validated`.
pub const PROMOTE_CONFIDENCE: f64 = 0.85;
/// Corroboration-count threshold for promotion to `Validated`.
pub const PROMOTE_CORROBORATION: u32 = 3;
/// Effective-confidence collapse threshold that contests an observation-grade fact.
pub const CONTEST_CONFIDENCE: f64 = 0.25;

/// Per-origin dynamics: prior `p0`, disuse half-life `h`, and the enum-conditioned
/// success/failure learning rates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OriginParams {
    pub prior: f64,
    pub half_life_days: f64,
    pub success_lr: f64,
    pub failure_lr: f64,
}

/// The `(p0, h, a+, a-)` parameters for an origin (spec §3.1). `Extracted` has
/// `failure_lr = 0`: a failure does not decrement it, it CONTESTS it (see
/// [`reclassify`]).
pub fn origin_params(origin: Origin) -> OriginParams {
    match origin {
        Origin::Extracted => OriginParams {
            prior: 0.90,
            half_life_days: 365.0,
            success_lr: 0.02,
            failure_lr: 0.0,
        },
        Origin::Inferred(InferredTier::Near) => OriginParams {
            prior: 0.65,
            half_life_days: 120.0,
            success_lr: 0.10,
            failure_lr: 0.15,
        },
        Origin::Inferred(InferredTier::Far) => OriginParams {
            prior: 0.50,
            half_life_days: 60.0,
            success_lr: 0.10,
            failure_lr: 0.15,
        },
        Origin::Ambiguous => OriginParams {
            prior: 0.35,
            half_life_days: 30.0,
            success_lr: 0.20,
            failure_lr: 0.20,
        },
        Origin::Validated => OriginParams {
            prior: 0.90,
            half_life_days: 365.0,
            success_lr: 0.02,
            failure_lr: 0.15,
        },
        Origin::Contested => OriginParams {
            prior: 0.20,
            half_life_days: 30.0,
            success_lr: 0.02,
            failure_lr: 0.15,
        },
    }
}

/// Multiplicative, enum-conditioned outcome update on the STORED confidence.
/// Success/Partial approach 1 with diminishing returns; Failure approaches 0.
/// Auto-bounded to `[0, 1]`.
pub fn apply_outcome_epistemic(confidence: f64, result: OutcomeResult, origin: Origin) -> f64 {
    let p = origin_params(origin);
    let c = confidence.clamp(0.0, 1.0);
    let updated = match result {
        OutcomeResult::Success => c + p.success_lr * (1.0 - c),
        // Partial is a weak success (preserves the old ~1:5 partial:success ratio).
        OutcomeResult::Partial => c + (p.success_lr * 0.2) * (1.0 - c),
        OutcomeResult::Failure => c - p.failure_lr * c,
    };
    updated.clamp(0.0, 1.0)
}

/// A single independent re-observation nudges confidence toward 1 by a small,
/// origin-independent amount — the passive signal that accrues even when no
/// explicit outcome is recorded.
pub fn apply_corroboration(confidence: f64) -> f64 {
    let c = confidence.clamp(0.0, 1.0);
    (c + CORROBORATION_LR * (1.0 - c)).clamp(0.0, 1.0)
}

/// Read-time effective confidence: relaxes the stored value TOWARD THE PRIOR
/// (not zero) at the origin's half-life — `effective = p0 + (c - p0)·2^(-dt/h)`.
/// Disuse erodes usage-earned trust, never the intrinsic prior. From above the
/// prior it stays `>= p0`; from below it stays `<= p0`; it never crosses.
pub fn effective_confidence_epistemic(
    confidence: f64,
    origin: Origin,
    last_applied: DateTime<Utc>,
    now: DateTime<Utc>,
) -> f64 {
    let p = origin_params(origin);
    let c = confidence.clamp(0.0, 1.0);
    let age_days = (now - last_applied).num_seconds() as f64 / 86_400.0;
    if age_days <= 0.0 {
        return c;
    }
    let factor = 0.5_f64.powf(age_days / p.half_life_days); // 2^(-dt/h)
    (p.prior + (c - p.prior) * factor).clamp(0.0, 1.0)
}

/// Reclassify an origin from usage (spec §3.4). Returns the possibly-new origin.
/// - `Inferred`/`Contested` -> `Validated` on sustained high conf + enough corroboration.
/// - `Extracted`/`Validated` -> `Contested` on a Failure or a confidence collapse.
pub fn reclassify(
    origin: Origin,
    effective_confidence: f64,
    corroboration: u32,
    last_result: Option<OutcomeResult>,
) -> Origin {
    let earned =
        effective_confidence >= PROMOTE_CONFIDENCE && corroboration >= PROMOTE_CORROBORATION;
    let contested = matches!(last_result, Some(OutcomeResult::Failure))
        || effective_confidence < CONTEST_CONFIDENCE;
    match origin {
        Origin::Inferred(_) if earned => Origin::Validated,
        // Rehabilitation: a re-confirmed Contested fact leaves quarantine.
        Origin::Contested if earned => Origin::Validated,
        Origin::Extracted | Origin::Validated if contested => Origin::Contested,
        other => other,
    }
}

/// Retrieval weight by origin. `Contested` is 0 — excluded from default retrieval
/// but kept on disk for audit.
pub fn origin_weight(origin: Origin) -> f64 {
    match origin {
        Origin::Extracted | Origin::Validated => 1.0,
        Origin::Inferred(InferredTier::Near) => 0.9,
        Origin::Inferred(InferredTier::Far) => 0.8,
        Origin::Ambiguous => 0.6,
        Origin::Contested => 0.0,
    }
}

/// Trust used for ranking = effective confidence scaled by origin weight.
pub fn trust(effective_confidence: f64, origin: Origin) -> f64 {
    (effective_confidence * origin_weight(origin)).clamp(0.0, 1.0)
}

/// Whether a fact is quarantined from default retrieval (kept for audit).
pub fn is_contested(origin: Origin) -> bool {
    matches!(origin, Origin::Contested)
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

    // ===== v-next epistemic model =====

    #[test]
    fn climb_under_repeated_success() {
        let origin = Origin::Inferred(InferredTier::Near);
        let start = origin_params(origin).prior; // 0.65
        let mut c = start;
        let mut prev = c;
        for _ in 0..5 {
            c = apply_outcome_epistemic(c, OutcomeResult::Success, origin);
            assert!(c > prev, "confidence must strictly climb: {c} !> {prev}");
            assert!((0.0..=1.0).contains(&c));
            prev = c;
        }
        assert!(c > start, "reinforced pattern must end above its prior");
    }

    #[test]
    fn relax_from_above_stays_at_or_above_prior() {
        let origin = Origin::Inferred(InferredTier::Near);
        let p0 = origin_params(origin).prior; // 0.65
        let now = Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();
        for days in [1_i64, 30, 120, 500, 5000] {
            let past = now - chrono::Duration::days(days);
            let eff = effective_confidence_epistemic(0.95, origin, past, now);
            assert!(
                eff >= p0 - 1e-9 && eff <= 0.95 + 1e-9,
                "days={days} eff={eff} must be in [p0, 0.95]"
            );
        }
        let far = now - chrono::Duration::days(100_000);
        let eff = effective_confidence_epistemic(0.95, origin, far, now);
        assert!(
            (eff - p0).abs() < 1e-3,
            "long disuse must approach prior: {eff}"
        );
    }

    #[test]
    fn relax_from_below_stays_at_or_below_prior() {
        let origin = Origin::Inferred(InferredTier::Near);
        let p0 = origin_params(origin).prior;
        let now = Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();
        for days in [1_i64, 30, 500] {
            let past = now - chrono::Duration::days(days);
            let eff = effective_confidence_epistemic(0.20, origin, past, now);
            assert!(
                eff >= 0.20 - 1e-9 && eff <= p0 + 1e-9,
                "days={days} eff={eff} must be in [0.20, p0]"
            );
        }
    }

    #[test]
    fn updates_stay_in_unit_interval() {
        let origin = Origin::Inferred(InferredTier::Near);
        let mut c = 0.5;
        for r in [
            OutcomeResult::Success,
            OutcomeResult::Failure,
            OutcomeResult::Failure,
            OutcomeResult::Partial,
            OutcomeResult::Success,
            OutcomeResult::Failure,
        ] {
            c = apply_outcome_epistemic(c, r, origin);
            assert!((0.0..=1.0).contains(&c), "out of bounds: {c}");
        }
        let mut c2 = 0.99;
        for _ in 0..20 {
            c2 = apply_corroboration(c2);
            assert!((0.0..=1.0).contains(&c2));
        }
    }

    #[test]
    fn reclassify_promotes_inferred_at_exact_boundary() {
        let inf = Origin::Inferred(InferredTier::Near);
        assert_eq!(reclassify(inf, 0.85, 3, None), Origin::Validated);
        assert_eq!(reclassify(inf, 0.849, 3, None), inf, "below conf threshold");
        assert_eq!(
            reclassify(inf, 0.85, 2, None),
            inf,
            "below corroboration threshold"
        );
    }

    #[test]
    fn reclassify_contests_observation_grade() {
        assert_eq!(
            reclassify(Origin::Extracted, 0.9, 0, Some(OutcomeResult::Failure)),
            Origin::Contested
        );
        assert_eq!(
            reclassify(Origin::Validated, 0.9, 5, Some(OutcomeResult::Failure)),
            Origin::Contested
        );
        assert_eq!(
            reclassify(Origin::Extracted, 0.2499, 0, None),
            Origin::Contested,
            "confidence collapse contests"
        );
        assert_eq!(
            reclassify(Origin::Extracted, 0.25, 0, None),
            Origin::Extracted,
            "exactly at threshold does NOT contest"
        );
        let inf = Origin::Inferred(InferredTier::Near);
        assert_eq!(
            reclassify(inf, 0.1, 0, None),
            inf,
            "Inferred is not auto-contested"
        );
    }

    #[test]
    fn contested_is_reversible() {
        assert_eq!(
            reclassify(Origin::Contested, 0.9, 3, None),
            Origin::Validated
        );
        assert_eq!(
            reclassify(Origin::Contested, 0.9, 2, None),
            Origin::Contested,
            "rehab needs corroboration"
        );
    }

    #[test]
    fn extracted_failure_contests_without_decrement() {
        // failure_lr = 0 for Extracted: value unchanged; the penalty is Contested.
        let c = apply_outcome_epistemic(0.9, OutcomeResult::Failure, Origin::Extracted);
        assert_eq!(c, 0.9);
        assert_eq!(
            reclassify(Origin::Extracted, c, 0, Some(OutcomeResult::Failure)),
            Origin::Contested
        );
    }

    #[test]
    fn trust_excludes_contested_and_ranks_by_origin() {
        assert_eq!(trust(0.9, Origin::Contested), 0.0);
        assert!(is_contested(Origin::Contested));
        assert!(!is_contested(Origin::Validated));
        assert!(
            trust(0.8, Origin::Extracted) > trust(0.8, Origin::Inferred(InferredTier::Far)),
            "observation-grade outranks a far inference at equal confidence"
        );
    }
}
