//! cortex-monitor (v4) — self-spectral metrics. Cortex observes its own
//! learning-graph spectrum over time and reports qualitative changes
//! (convergence, bifurcation, ξ_cross approach).
//!
//! Status: SCAFFOLD — types and persistence only. Detection logic is
//! deliberately stubbed because the thresholds need calibration from
//! actual spectrum history — the spec calls this out, and per the v4
//! plan-of-record this crate ships *before* the detection logic so we
//! can collect history first.
//!
//! ## Persistence
//!
//! - `<state-root>/spectrum-history/snapshot-{rfc3339-z}.json` — one per
//!   dreaming pass.
//! - History is append-only, never rewritten. Detection runs over windows
//!   of consecutive snapshots.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpectrumSnapshot {
    pub snapshot_id: String,
    pub timestamp: String,
    pub n_nodes: usize,
    pub k_modes: usize,
    pub eigenvalues: Vec<f64>,
    /// `λ₁ − λ₂` (gap between top two eigenvalues). Closing gap signals
    /// approach to ξ_cross / phase transition.
    pub spectral_gap: f64,
    /// Magnitude of the dominant eigenvalue.
    pub dominant_magnitude: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SpectrumHistory {
    pub snapshots: Vec<SpectrumSnapshot>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum QualitativeState {
    /// Spectrum stabilizing across consecutive snapshots — cortex has
    /// converged on its current learnings.
    Converged,
    /// Eigenvalue crossing detected — a structural reorganization happened.
    Bifurcation,
    /// Spectral gap collapsing toward zero — approaching a phase
    /// transition. Worth investigating before more learnings land.
    ApproachingXiCross,
    /// No clear signal yet (insufficient snapshots, or still mid-trajectory).
    Indeterminate,
}

pub fn spectrum_history_dir(state_root: &Path) -> PathBuf {
    state_root.join("spectrum-history")
}

/// **STUB** — record a new spectrum snapshot to disk.
pub fn record_snapshot(_state_root: &Path, _snapshot: &SpectrumSnapshot) -> anyhow::Result<()> {
    anyhow::bail!("record_snapshot: not yet implemented (v4 Phase 3 stub)")
}

/// **STUB** — load all snapshots from disk in chronological order.
pub fn load_history(_state_root: &Path) -> anyhow::Result<SpectrumHistory> {
    Ok(SpectrumHistory::default())
}

/// **STUB** — classify the current trajectory given the recent history.
/// Thresholds left as parameters because they need calibration; the v4
/// plan-of-record gates the actual classification logic on having ≥3
/// real snapshots first.
pub fn classify_trajectory(
    _history: &SpectrumHistory,
    _convergence_eps: f64,
    _bifurcation_min_jump: f64,
    _xi_cross_gap_threshold: f64,
) -> QualitativeState {
    QualitativeState::Indeterminate
}

/// Convenience: build a `SpectrumSnapshot` from an Eigendecomposition.
/// Pure / no I/O so it can be tested without disk.
pub fn snapshot_from_eigendecomposition(
    eigendecomp: &cortex_spectral::Eigendecomposition,
    timestamp: String,
    snapshot_id: String,
) -> SpectrumSnapshot {
    let eigenvalues: Vec<f64> = eigendecomp.modes.iter().map(|m| m.eigenvalue).collect();
    let dominant_magnitude = eigenvalues.first().copied().unwrap_or(0.0).abs();
    let spectral_gap = match (eigenvalues.first(), eigenvalues.get(1)) {
        (Some(a), Some(b)) => a - b,
        _ => 0.0,
    };
    SpectrumSnapshot {
        snapshot_id,
        timestamp,
        n_nodes: eigendecomp.n_nodes,
        k_modes: eigendecomp.modes.len(),
        eigenvalues,
        spectral_gap,
        dominant_magnitude,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cortex_spectral::{Eigendecomposition, Eigenmode, SolverKind};

    #[test]
    fn snapshot_extracts_gap_and_magnitude() {
        let decomp = Eigendecomposition {
            modes: vec![
                Eigenmode {
                    eigenvalue: 4.0,
                    eigenvector: vec![],
                },
                Eigenmode {
                    eigenvalue: 1.0,
                    eigenvector: vec![],
                },
            ],
            solver: SolverKind::Dense,
            n_nodes: 2,
        };
        let s = snapshot_from_eigendecomposition(&decomp, "ts".into(), "id".into());
        assert_eq!(s.dominant_magnitude, 4.0);
        assert_eq!(s.spectral_gap, 3.0);
        assert_eq!(s.k_modes, 2);
    }

    #[test]
    fn empty_eigendecomp_is_zero() {
        let decomp = Eigendecomposition {
            modes: vec![],
            solver: SolverKind::Dense,
            n_nodes: 0,
        };
        let s = snapshot_from_eigendecomposition(&decomp, "ts".into(), "id".into());
        assert_eq!(s.dominant_magnitude, 0.0);
        assert_eq!(s.spectral_gap, 0.0);
    }

    #[test]
    fn classify_returns_indeterminate_until_implemented() {
        let h = SpectrumHistory::default();
        assert_eq!(
            classify_trajectory(&h, 0.01, 0.5, 0.05),
            QualitativeState::Indeterminate
        );
    }
}
