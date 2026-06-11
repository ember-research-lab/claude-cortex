//! cortex-confidence — durable, decaying **per-behavior** confidence.
//!
//! This is the trust signal behind *progressive autonomy*: how proven is a given behavior (an agent
//! doing an action-class), computed from its recorded outcomes. It is the durable backing the SMB
//! platform's `ConfidenceOracle` was waiting on — distinct from cortex's *learning* confidence (which
//! is per ledger-block), but built on the **same math**: `cortex_core::confidence`'s Success / Partial
//! / Failure deltas + 180-day half-life decay.
//!
//! ## Behavior key — opaque, consumer-composed
//! The store keys by an opaque `&str` behavior id. cortex stays **neutral** about its structure: a
//! consumer composes it from whatever identifies the behavior (e.g. `"agent:reception\x1f"` +
//! `"appointment_reminder"`). The key may also encode a **scope** prefix (`shared:…` vs
//! `user:alice:…`) per the multi-user ledger model
//! (`ember-smb-platform/design/shared-ledger-scoping.md`) — the engine doesn't need to know.
//!
//! ## A derived view, not a source of truth
//! The confidence store is a **materialized, decaying view** over outcomes. The outcomes themselves
//! live in the action ledger (the source of truth); this store can be rebuilt by replaying them. So it
//! is *state* (it goes stale / decays), not an immutable ledger fact — it persists like a handoff
//! (atomic JSON), not like a signed block.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use cortex_core::confidence::{apply_outcome_delta, decay_confidence};
use cortex_core::models::OutcomeResult;
use cortex_core::persist::write_atomic_json;
use cortex_core::time::UtcTime;
use serde::{Deserialize, Serialize};

/// One behavior's confidence state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfidenceState {
    /// Confidence in `[0, 1]` **as of `last_applied`** (a reader decays it to "now").
    pub value: f64,
    /// How many outcomes have fed this behavior (the evidence count — a promotion policy wants both a
    /// high value AND enough observations).
    pub observations: u32,
    /// When `value` was last updated (the decay anchor).
    pub last_applied: UtcTime,
}

/// The per-behavior confidence store: `behavior id → state`. `BTreeMap` for deterministic
/// serialization.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BehaviorConfidence {
    #[serde(default)]
    states: BTreeMap<String, ConfidenceState>,
}

impl BehaviorConfidence {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record an outcome for `behavior` at `now`. **Decay-then-delta:** the stored value is first
    /// decayed to `now` (so a long-idle behavior's old trust has faded), then the outcome's delta is
    /// applied and `observations` is bumped. A never-seen behavior starts at `0.0` before its first
    /// delta (so one success → `0.10`, matching learning confidence).
    pub fn record(&mut self, behavior: &str, result: OutcomeResult, now: DateTime<Utc>) {
        let prior = self.states.get(behavior);
        let decayed = match prior {
            Some(s) => decay_confidence(s.value, s.last_applied.0, now),
            None => 0.0,
        };
        let value = apply_outcome_delta(decayed, result);
        let observations = prior.map(|s| s.observations).unwrap_or(0) + 1;
        self.states.insert(
            behavior.to_string(),
            ConfidenceState {
                value,
                observations,
                last_applied: now.into(),
            },
        );
    }

    /// The confidence for `behavior` **decayed to `now`** + its observation count, or `None` if the
    /// behavior has never been observed (unproven-because-untried, distinct from tried-and-low).
    pub fn confidence(&self, behavior: &str, now: DateTime<Utc>) -> Option<(f64, u32)> {
        self.states.get(behavior).map(|s| {
            (
                decay_confidence(s.value, s.last_applied.0, now),
                s.observations,
            )
        })
    }

    /// Number of distinct behaviors tracked.
    pub fn len(&self) -> usize {
        self.states.len()
    }

    pub fn is_empty(&self) -> bool {
        self.states.is_empty()
    }
}

/// On-disk path for the confidence store under a cortex state root.
pub fn confidence_path(state_root: &Path) -> PathBuf {
    state_root.join("confidence").join("behaviors.json")
}

/// Load the store, or an empty one if none exists yet.
pub fn load(state_root: &Path) -> anyhow::Result<BehaviorConfidence> {
    let path = confidence_path(state_root);
    if !path.is_file() {
        return Ok(BehaviorConfidence::new());
    }
    let bytes = std::fs::read(&path)?;
    Ok(serde_json::from_slice(&bytes)?)
}

/// Persist the store atomically (temp + rename, like the handoff store).
pub fn save(state_root: &Path, store: &BehaviorConfidence) -> anyhow::Result<()> {
    write_atomic_json(&confidence_path(state_root), store).map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use tempfile::TempDir;

    fn t(y: i32, mo: u32, d: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, 0, 0, 0).unwrap()
    }

    #[test]
    fn unseen_behavior_has_no_confidence() {
        let bc = BehaviorConfidence::new();
        assert_eq!(bc.confidence("agent:a\x1fsend", t(2026, 6, 11)), None);
    }

    #[test]
    fn first_success_starts_from_zero() {
        let mut bc = BehaviorConfidence::new();
        let now = t(2026, 6, 11);
        bc.record("b", OutcomeResult::Success, now);
        let (v, obs) = bc.confidence("b", now).unwrap();
        assert!((v - 0.10).abs() < 1e-9, "got {v}"); // 0 + 0.10
        assert_eq!(obs, 1);
    }

    #[test]
    fn outcomes_accumulate_and_failure_drags_down() {
        let mut bc = BehaviorConfidence::new();
        let now = t(2026, 6, 11);
        for _ in 0..3 {
            bc.record("b", OutcomeResult::Success, now); // 0.10, 0.20, 0.30
        }
        bc.record("b", OutcomeResult::Failure, now); // 0.30 - 0.15 = 0.15
        let (v, obs) = bc.confidence("b", now).unwrap();
        assert!((v - 0.15).abs() < 1e-9, "got {v}");
        assert_eq!(obs, 4);
    }

    #[test]
    fn record_decays_old_value_before_applying_delta() {
        let mut bc = BehaviorConfidence::new();
        let t0 = t(2026, 1, 1);
        bc.record("b", OutcomeResult::Success, t0); // 0.10 at t0
                                                    // 180 days later, the 0.10 has halved to 0.05, then +0.10 success → 0.15.
        let t1 = t0 + chrono::Duration::days(180);
        bc.record("b", OutcomeResult::Success, t1);
        let (v, _) = bc.confidence("b", t1).unwrap();
        assert!((v - 0.15).abs() < 1e-9, "got {v}");
    }

    #[test]
    fn confidence_decays_on_read() {
        let mut bc = BehaviorConfidence::new();
        let t0 = t(2026, 1, 1);
        // Build up to 0.60 (six successes at the same instant).
        for _ in 0..6 {
            bc.record("b", OutcomeResult::Success, t0);
        }
        let (v0, _) = bc.confidence("b", t0).unwrap();
        assert!((v0 - 0.60).abs() < 1e-9, "got {v0}");
        // Half a half-life-year later it has halved.
        let (v1, _) = bc
            .confidence("b", t0 + chrono::Duration::days(180))
            .unwrap();
        assert!((v1 - 0.30).abs() < 1e-9, "got {v1}");
    }

    #[test]
    fn persists_and_reloads() {
        let dir = TempDir::new().unwrap();
        let now = t(2026, 6, 11);
        let mut bc = BehaviorConfidence::new();
        bc.record(
            "agent:reception\x1fappointment_reminder",
            OutcomeResult::Success,
            now,
        );
        bc.record(
            "agent:reception\x1fappointment_reminder",
            OutcomeResult::Success,
            now,
        );
        save(dir.path(), &bc).unwrap();

        let loaded = load(dir.path()).unwrap();
        let (v, obs) = loaded
            .confidence("agent:reception\x1fappointment_reminder", now)
            .unwrap();
        assert!((v - 0.20).abs() < 1e-9, "got {v}");
        assert_eq!(obs, 2);
        assert_eq!(loaded.len(), 1);
    }

    #[test]
    fn load_missing_is_empty() {
        let dir = TempDir::new().unwrap();
        assert!(load(dir.path()).unwrap().is_empty());
    }
}
