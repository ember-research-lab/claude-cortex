//! Handoff substrate — work-in-progress state capture for cross-session
//! continuity.
//!
//! ## Why this is a separate crate from `cortex-core`
//!
//! v0.3.6 surfaced the "pattern vs state" distinction during real-data
//! audit: the long-term ledger is for durable facts (discoveries,
//! decisions, errors, patterns) that survive over time. **State**
//! (current file counts, draft versions, pending tasks, current
//! blockers) goes stale within days/weeks and shouldn't pollute the
//! immutable substrate. Handoffs are the right home for state.
//!
//! ## Persistence
//!
//! - `<state-root>/handoffs/handoff-{rfc3339-z}.json` — one per save.
//!   Append-only; never overwritten.
//! - `<state-root>/handoffs/current` — pointer file holding the
//!   filename of the latest handoff. Atomic temp+rename update so
//!   readers never observe a partial write.
//! - List, latest, and per-session retrieval supported.
//!
//! ## What goes in a handoff
//!
//! Mirrors v2 cortex's design (which was good): completed tasks,
//! pending tasks, blockers, modified files, free-form context notes,
//! plus session_id + timestamp metadata. All optional except session
//! identity and timestamp.

use std::fs;
use std::path::{Path, PathBuf};

use cortex_core::time::UtcTime;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Handoff {
    pub handoff_id: String,
    pub session_id: String,
    pub timestamp: UtcTime,
    #[serde(default)]
    pub completed_tasks: Vec<String>,
    #[serde(default)]
    pub pending_tasks: Vec<String>,
    #[serde(default)]
    pub blockers: Vec<String>,
    #[serde(default)]
    pub modified_files: Vec<String>,
    #[serde(default)]
    pub context_notes: String,
}

impl Handoff {
    pub fn new(session_id: impl Into<String>) -> Self {
        Self {
            handoff_id: Uuid::new_v4().to_string(),
            session_id: session_id.into(),
            timestamp: UtcTime::now(),
            completed_tasks: Vec::new(),
            pending_tasks: Vec::new(),
            blockers: Vec::new(),
            modified_files: Vec::new(),
            context_notes: String::new(),
        }
    }

    pub fn with_completed(mut self, tasks: impl IntoIterator<Item = String>) -> Self {
        self.completed_tasks = tasks.into_iter().collect();
        self
    }

    pub fn with_pending(mut self, tasks: impl IntoIterator<Item = String>) -> Self {
        self.pending_tasks = tasks.into_iter().collect();
        self
    }

    pub fn with_blockers(mut self, blockers: impl IntoIterator<Item = String>) -> Self {
        self.blockers = blockers.into_iter().collect();
        self
    }

    pub fn with_modified_files(mut self, files: impl IntoIterator<Item = String>) -> Self {
        self.modified_files = files.into_iter().collect();
        self
    }

    pub fn with_context(mut self, notes: impl Into<String>) -> Self {
        self.context_notes = notes.into();
        self
    }
}

pub fn handoffs_dir(state_root: &Path) -> PathBuf {
    state_root.join("handoffs")
}

pub fn current_pointer_path(state_root: &Path) -> PathBuf {
    handoffs_dir(state_root).join("current")
}

fn handoff_filename(h: &Handoff) -> String {
    let safe_ts = h.timestamp.as_str().replace(':', "-");
    format!("handoff-{safe_ts}.json")
}

/// Write a handoff to disk + atomically advance the `current` pointer.
/// Old handoffs persist (append-only). The pointer is the only mutable
/// slot, updated via temp+rename.
pub fn record_handoff(state_root: &Path, h: &Handoff) -> anyhow::Result<PathBuf> {
    let dir = handoffs_dir(state_root);
    fs::create_dir_all(&dir)?;
    let filename = handoff_filename(h);
    let target = dir.join(&filename);
    let tmp_target = dir.join(format!("{filename}.{}.tmp", Uuid::new_v4().simple()));
    let bytes = serde_json::to_vec_pretty(h)?;
    fs::write(&tmp_target, &bytes)?;
    if let Err(e) = fs::rename(&tmp_target, &target) {
        let _ = fs::remove_file(&tmp_target);
        return Err(e.into());
    }

    let current = current_pointer_path(state_root);
    let tmp_pointer = dir.join(format!("current.{}.tmp", Uuid::new_v4().simple()));
    fs::write(&tmp_pointer, filename.as_bytes())?;
    if let Err(e) = fs::rename(&tmp_pointer, &current) {
        let _ = fs::remove_file(&tmp_pointer);
        return Err(e.into());
    }

    Ok(target)
}

/// Read the most-recent handoff (whichever `current` points at). Returns
/// `None` if no current pointer exists. Errors only on I/O / parse
/// failure of an existing pointer or handoff.
pub fn read_current(state_root: &Path) -> anyhow::Result<Option<Handoff>> {
    let pointer = current_pointer_path(state_root);
    if !pointer.is_file() {
        return Ok(None);
    }
    let filename = fs::read_to_string(&pointer)?.trim().to_string();
    if filename.is_empty() {
        return Ok(None);
    }
    let path = handoffs_dir(state_root).join(&filename);
    if !path.is_file() {
        return Ok(None);
    }
    let bytes = fs::read(&path)?;
    Ok(Some(serde_json::from_slice(&bytes)?))
}

/// Load all handoffs in chronological order (oldest first). Filenames
/// embed the timestamp so a name-sort gives chronological order;
/// corrupt files are skipped with a warning rather than aborting.
pub fn list_handoffs(state_root: &Path) -> anyhow::Result<Vec<Handoff>> {
    let dir = handoffs_dir(state_root);
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut entries: Vec<(String, PathBuf)> = Vec::new();
    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.starts_with("handoff-") || !name.ends_with(".json") {
            continue;
        }
        entries.push((name.to_string(), path));
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let mut out = Vec::with_capacity(entries.len());
    for (_, path) in entries {
        match fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<Handoff>(&bytes) {
                Ok(h) => out.push(h),
                Err(e) => eprintln!(
                    "cortex-handoff: skipping corrupt handoff {}: {e}",
                    path.display()
                ),
            },
            Err(e) => eprintln!(
                "cortex-handoff: cannot read {}: {e}",
                path.display()
            ),
        }
    }
    Ok(out)
}

/// Find the most-recent handoff for a specific session_id. Useful for
/// resuming a specific prior session by id, vs the global `current`
/// which is the latest handoff regardless of session.
pub fn latest_for_session(
    state_root: &Path,
    session_id: &str,
) -> anyhow::Result<Option<Handoff>> {
    let all = list_handoffs(state_root)?;
    Ok(all.into_iter().rev().find(|h| h.session_id == session_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn paths_are_stable() {
        let root = Path::new("/tmp/cortex-state");
        assert_eq!(
            handoffs_dir(root),
            PathBuf::from("/tmp/cortex-state/handoffs")
        );
        assert_eq!(
            current_pointer_path(root),
            PathBuf::from("/tmp/cortex-state/handoffs/current")
        );
    }

    #[test]
    fn read_current_on_missing_state_returns_none() {
        let dir = TempDir::new().unwrap();
        assert!(read_current(dir.path()).unwrap().is_none());
    }

    #[test]
    fn record_then_read_round_trips() {
        let dir = TempDir::new().unwrap();
        let h = Handoff::new("session-abc")
            .with_completed(vec!["task A".into(), "task B".into()])
            .with_pending(vec!["task C".into()])
            .with_blockers(vec!["waiting on credentials".into()])
            .with_modified_files(vec!["src/foo.rs".into()])
            .with_context("Working on auth refactor; pause point chosen for safety");
        let path = record_handoff(dir.path(), &h).unwrap();
        assert!(path.is_file());
        let loaded = read_current(dir.path()).unwrap().expect("no current");
        assert_eq!(loaded.session_id, "session-abc");
        assert_eq!(loaded.completed_tasks.len(), 2);
        assert_eq!(loaded.pending_tasks, vec!["task C"]);
        assert_eq!(loaded.blockers, vec!["waiting on credentials"]);
        assert_eq!(loaded.modified_files, vec!["src/foo.rs"]);
        assert!(loaded.context_notes.contains("auth refactor"));
    }

    #[test]
    fn current_advances_through_multiple_records() {
        let dir = TempDir::new().unwrap();
        record_handoff(
            dir.path(),
            &Handoff::new("s1").with_pending(vec!["first".into()]),
        )
        .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        record_handoff(
            dir.path(),
            &Handoff::new("s2").with_pending(vec!["second".into()]),
        )
        .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        record_handoff(
            dir.path(),
            &Handoff::new("s3").with_pending(vec!["third".into()]),
        )
        .unwrap();
        let current = read_current(dir.path()).unwrap().expect("no current");
        assert_eq!(current.session_id, "s3");
        assert_eq!(current.pending_tasks, vec!["third"]);
        // All three files persist (append-only).
        let all = list_handoffs(dir.path()).unwrap();
        assert_eq!(all.len(), 3);
        // Chronological order.
        assert_eq!(all[0].session_id, "s1");
        assert_eq!(all[2].session_id, "s3");
    }

    #[test]
    fn latest_for_session_finds_most_recent_per_session() {
        let dir = TempDir::new().unwrap();
        record_handoff(
            dir.path(),
            &Handoff::new("alpha").with_context("first alpha"),
        )
        .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        record_handoff(
            dir.path(),
            &Handoff::new("beta").with_context("first beta"),
        )
        .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        record_handoff(
            dir.path(),
            &Handoff::new("alpha").with_context("second alpha"),
        )
        .unwrap();

        let latest_alpha = latest_for_session(dir.path(), "alpha")
            .unwrap()
            .expect("no alpha handoff found");
        assert_eq!(latest_alpha.context_notes, "second alpha");

        let latest_beta = latest_for_session(dir.path(), "beta")
            .unwrap()
            .expect("no beta handoff found");
        assert_eq!(latest_beta.context_notes, "first beta");

        assert!(latest_for_session(dir.path(), "missing").unwrap().is_none());
    }

    #[test]
    fn list_handoffs_skips_non_handoff_files() {
        let dir = TempDir::new().unwrap();
        let hd = handoffs_dir(dir.path());
        std::fs::create_dir_all(&hd).unwrap();
        std::fs::write(hd.join("README.md"), "not a handoff").unwrap();
        std::fs::write(hd.join("handoff-foo.txt"), "wrong ext").unwrap();
        std::fs::write(hd.join("other.json"), "wrong prefix").unwrap();
        record_handoff(dir.path(), &Handoff::new("real")).unwrap();
        let all = list_handoffs(dir.path()).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].session_id, "real");
    }
}
