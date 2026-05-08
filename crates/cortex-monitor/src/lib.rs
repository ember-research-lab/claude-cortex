//! cortex-monitor (v4) — self-spectral metrics. Cortex observes its own
//! learning-graph spectrum over time and reports qualitative changes
//! (convergence, bifurcation, ξ_cross approach).
//!
//! ## Persistence
//!
//! - `<state-root>/spectrum-history/snapshot-{rfc3339-z}.json` — one per
//!   dreaming pass, immutable after write.
//! - History is append-only, never rewritten. Detection runs over windows
//!   of consecutive snapshots.
//!
//! ## Phase 3 status
//!
//! Persistence layer is implemented (record + load round-trip). Detection
//! logic (`classify_trajectory`) is intentionally left returning
//! `Indeterminate` until ≥3 real snapshots exist to calibrate the
//! thresholds against. Per the v4 plan-of-record this is the
//! introspection-first ordering: collect history before tightening rules.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

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

fn snapshot_filename(snapshot: &SpectrumSnapshot) -> String {
    // Path-safe: replace `:` (illegal on Windows) with `-`. Keep `Z` so
    // chronological sort still works on the resulting filename strings.
    let safe_ts = snapshot.timestamp.replace(':', "-");
    format!("snapshot-{}.json", safe_ts)
}

/// Record a new spectrum snapshot to disk via atomic temp+rename so
/// concurrent dreaming passes can't corrupt each other's output.
pub fn record_snapshot(state_root: &Path, snapshot: &SpectrumSnapshot) -> anyhow::Result<()> {
    let dir = spectrum_history_dir(state_root);
    fs::create_dir_all(&dir)?;
    let target = dir.join(snapshot_filename(snapshot));
    let tmp = dir.join(format!(
        "{}.{}.tmp",
        snapshot_filename(snapshot),
        Uuid::new_v4().simple()
    ));
    let bytes = serde_json::to_vec_pretty(snapshot)?;
    fs::write(&tmp, &bytes)?;
    if let Err(e) = fs::rename(&tmp, &target) {
        let _ = fs::remove_file(&tmp);
        return Err(e.into());
    }
    Ok(())
}

/// Load every snapshot under `<state-root>/spectrum-history/` in
/// chronological order (sorted by timestamp). Files that fail to parse
/// are skipped with a warning rather than aborting — a single corrupt
/// snapshot shouldn't break analysis of the rest.
pub fn load_history(state_root: &Path) -> anyhow::Result<SpectrumHistory> {
    let dir = spectrum_history_dir(state_root);
    if !dir.is_dir() {
        return Ok(SpectrumHistory::default());
    }
    let mut snapshots = Vec::new();
    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.starts_with("snapshot-") || !name.ends_with(".json") {
            continue;
        }
        match fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<SpectrumSnapshot>(&bytes) {
                Ok(s) => snapshots.push(s),
                Err(e) => eprintln!(
                    "cortex-monitor: skipping corrupt snapshot {}: {e}",
                    path.display()
                ),
            },
            Err(e) => eprintln!("cortex-monitor: cannot read {}: {e}", path.display()),
        }
    }
    snapshots.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
    Ok(SpectrumHistory { snapshots })
}

/// Classify the trajectory of recent spectra.
///
/// Stays `Indeterminate` until the v4 plan-of-record's calibration gate
/// (≥3 real snapshots from real dreaming passes) has been met. The
/// thresholds are left as parameters so the detection logic can be
/// trialed against a fixed threshold sweep once history exists, without
/// changing the signature later.
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
    use tempfile::TempDir;

    fn snap(ts: &str, dom: f64, gap: f64) -> SpectrumSnapshot {
        SpectrumSnapshot {
            snapshot_id: format!("id-{ts}"),
            timestamp: ts.to_string(),
            n_nodes: 5,
            k_modes: 3,
            eigenvalues: vec![dom, dom - gap, 0.0],
            spectral_gap: gap,
            dominant_magnitude: dom,
        }
    }

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

    #[test]
    fn load_history_on_missing_dir_returns_empty() {
        let dir = TempDir::new().unwrap();
        let history = load_history(dir.path()).unwrap();
        assert_eq!(history.snapshots.len(), 0);
    }

    #[test]
    fn record_then_load_round_trips_snapshot() {
        let dir = TempDir::new().unwrap();
        let s = snap("2026-05-07T22-15-00.000000Z", 3.5, 1.2);
        record_snapshot(dir.path(), &s).unwrap();

        let history = load_history(dir.path()).unwrap();
        assert_eq!(history.snapshots.len(), 1);
        let loaded = &history.snapshots[0];
        assert_eq!(loaded.snapshot_id, s.snapshot_id);
        assert_eq!(loaded.timestamp, s.timestamp);
        assert_eq!(loaded.n_nodes, s.n_nodes);
        assert!((loaded.spectral_gap - s.spectral_gap).abs() < 1e-12);
        assert!((loaded.dominant_magnitude - s.dominant_magnitude).abs() < 1e-12);
    }

    #[test]
    fn load_history_returns_chronological_order() {
        let dir = TempDir::new().unwrap();
        // Record out of order; load_history must sort.
        record_snapshot(dir.path(), &snap("2026-05-07T22-30-00.000000Z", 2.0, 1.0)).unwrap();
        record_snapshot(dir.path(), &snap("2026-05-07T22-10-00.000000Z", 1.0, 0.5)).unwrap();
        record_snapshot(dir.path(), &snap("2026-05-07T22-20-00.000000Z", 1.5, 0.8)).unwrap();
        let history = load_history(dir.path()).unwrap();
        assert_eq!(history.snapshots.len(), 3);
        let ts: Vec<&str> = history
            .snapshots
            .iter()
            .map(|s| s.timestamp.as_str())
            .collect();
        assert_eq!(
            ts,
            vec![
                "2026-05-07T22-10-00.000000Z",
                "2026-05-07T22-20-00.000000Z",
                "2026-05-07T22-30-00.000000Z",
            ]
        );
    }

    #[test]
    fn append_only_no_overwrite_on_distinct_timestamps() {
        let dir = TempDir::new().unwrap();
        record_snapshot(dir.path(), &snap("2026-05-07T22-30-00.000000Z", 2.0, 1.0)).unwrap();
        record_snapshot(dir.path(), &snap("2026-05-07T22-31-00.000000Z", 2.5, 0.8)).unwrap();
        record_snapshot(dir.path(), &snap("2026-05-07T22-32-00.000000Z", 3.0, 0.4)).unwrap();
        let history = load_history(dir.path()).unwrap();
        assert_eq!(history.snapshots.len(), 3);
        // Spectral gap should be monotonically decreasing in this trajectory.
        for w in history.snapshots.windows(2) {
            assert!(w[0].spectral_gap >= w[1].spectral_gap);
        }
    }

    #[test]
    fn load_history_skips_non_snapshot_files() {
        let dir = TempDir::new().unwrap();
        let snapshot_dir = spectrum_history_dir(dir.path());
        std::fs::create_dir_all(&snapshot_dir).unwrap();
        // Distractor files that should be ignored.
        std::fs::write(snapshot_dir.join("README.md"), "not a snapshot").unwrap();
        std::fs::write(snapshot_dir.join("snapshot-foo.txt"), "wrong ext").unwrap();
        std::fs::write(snapshot_dir.join("other.json"), "wrong prefix").unwrap();
        // A real snapshot.
        record_snapshot(dir.path(), &snap("2026-05-07T22-30-00.000000Z", 2.0, 1.0)).unwrap();
        let history = load_history(dir.path()).unwrap();
        assert_eq!(history.snapshots.len(), 1);
    }
}
