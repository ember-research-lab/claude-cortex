//! Active memory layer (v4) — curated working set built from the top-k
//! dominant eigenmodes of the learning-graph Laplacian.
//!
//! Status: SCAFFOLD — types and persistence layout only. Eigenmode-driven
//! `build_active_memory` is deferred to Phase 4 impl.
//!
//! ## Persistence
//!
//! - `<state-root>/active/active-{rfc3339-z}.json` — immutable snapshot.
//! - `<state-root>/active/current` — symlink (or pointer file on Windows)
//!   to the latest snapshot. Updated atomically via tempfile + rename.
//! - Old snapshots accumulate; v4 keeps the last N (config TBD) for
//!   spectrum stability analysis by cortex-monitor.

use std::path::{Path, PathBuf};

use cortex_spectral::NodeId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveMemory {
    pub snapshot_id: String,
    pub timestamp: String,
    /// Hashes of ledger blocks the snapshot was built from. Lets cortex-monitor
    /// know which spectrum corresponds to which ledger range.
    pub source_block_hashes: Vec<String>,
    pub eigenmode_count: usize,
    pub entries: Vec<ActiveEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveEntry {
    pub node: NodeId,
    pub learning_id: String,
    pub projection_weight: f64,
}

pub fn active_memory_dir(state_root: &Path) -> PathBuf {
    state_root.join("active")
}

pub fn current_pointer_path(state_root: &Path) -> PathBuf {
    active_memory_dir(state_root).join("current")
}

/// **STUB** — build active memory from a graph + eigendecomposition + k.
pub fn build_active_memory(
    _graph: &cortex_spectral::LearningGraph,
    _eigendecomp: &cortex_spectral::Eigendecomposition,
    _k: usize,
) -> anyhow::Result<ActiveMemory> {
    anyhow::bail!("build_active_memory: not yet implemented (v4 Phase 4 stub)")
}

/// **STUB** — write a snapshot to disk and update `current`.
pub fn write_snapshot(_state_root: &Path, _snapshot: &ActiveMemory) -> anyhow::Result<()> {
    anyhow::bail!("write_snapshot: not yet implemented (v4 Phase 4 stub)")
}

/// **STUB** — read whichever snapshot `current` points at.
pub fn read_current(_state_root: &Path) -> anyhow::Result<Option<ActiveMemory>> {
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_stable() {
        let root = Path::new("/tmp/cortex-state");
        assert_eq!(
            active_memory_dir(root),
            PathBuf::from("/tmp/cortex-state/active")
        );
        assert_eq!(
            current_pointer_path(root),
            PathBuf::from("/tmp/cortex-state/active/current")
        );
    }
}
