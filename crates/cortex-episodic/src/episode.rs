//! `EpisodeRecord` and `EpisodeStatus` — the core types for an episodic
//! memory entry.

use cortex_core::time::{format_rfc3339_z, UtcTime};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Lifecycle status of an episode record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EpisodeStatus {
    /// Captured but not yet processed by the consolidator.
    Unconsolidated,
    /// The consolidator is actively processing this episode (guard state).
    Consolidating,
    /// Consolidator has promoted learnings; awaiting outcome confirmation.
    ConsolidatedPendingConfirmation,
    /// All promoted blocks are confirmed (or TTL elapsed); safe to prune.
    Evictable,
}

/// A single captured episode: a transcript tail slice plus provenance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpisodeRecord {
    /// Unique episode identifier (UUID v4).
    pub episode_id: String,
    /// The Claude Code session that produced this episode.
    pub session_id: String,
    /// When this episode was captured.
    pub captured_at: UtcTime,
    /// How the capture was triggered.
    /// Typical values: `"precompact:auto"`, `"precompact:manual"`,
    /// `"sessionend:<reason>"`.
    pub capture_source: String,
    /// Absolute path to the transcript file, if available.
    pub transcript_path: Option<String>,
    /// `[start_offset, end_offset]` byte range within `transcript_path`.
    /// `[0, 0]` when no transcript is available.
    pub byte_range: [u64; 2],
    /// Lifecycle status.
    pub status: EpisodeStatus,
    /// Block IDs promoted into the ledger from this episode.
    #[serde(default)]
    pub promoted_block_ids: Vec<String>,
    /// Custom instructions active at capture time, if captured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_instructions: Option<String>,
}

impl EpisodeRecord {
    /// Construct a new `EpisodeRecord` in the `Unconsolidated` state.
    pub fn new(
        session_id: impl Into<String>,
        capture_source: impl Into<String>,
        transcript_path: Option<String>,
        byte_range: [u64; 2],
    ) -> Self {
        Self {
            episode_id: Uuid::new_v4().to_string(),
            session_id: session_id.into(),
            captured_at: UtcTime::now(),
            capture_source: capture_source.into(),
            transcript_path,
            byte_range,
            status: EpisodeStatus::Unconsolidated,
            promoted_block_ids: Vec::new(),
            custom_instructions: None,
        }
    }

    /// Filename for this episode: `episode-{session_id}-{rfc3339-z-safe}.json`.
    ///
    /// Uses the same `:` → `-` replacement as `cortex-handoff` so the filename
    /// is safe on all platforms and sorts chronologically.
    pub fn filename(&self) -> String {
        let safe_ts = format_rfc3339_z(&self.captured_at.into_inner()).replace(':', "-");
        format!("episode-{}-{safe_ts}.json", self.session_id)
    }
}
