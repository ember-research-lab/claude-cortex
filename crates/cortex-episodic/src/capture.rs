//! `capture_tail` — record the byte RANGE of the new transcript tail
//! (watermark→EOF) WITHOUT reading the contents, and write a new
//! `EpisodeRecord` to disk. The consolidator reads the actual bytes later
//! using the stored `byte_range`.

use std::path::Path;

use crate::episode::EpisodeRecord;
use crate::manifest::{episodic_dir, load_manifest, save_manifest};

/// Record the byte range of the new transcript tail (watermark→EOF) for
/// `session_id`, write the episode JSON, and advance the watermark in
/// the manifest. Does NOT read the transcript contents — the byte range
/// is stored so the consolidator can read the bytes later.
///
/// Returns `None` when there are no new bytes past the watermark
/// (idempotent no-op). Returns `Some(episode)` on a successful capture.
///
/// When `transcript_path` is `None`, the episode is written with
/// `byte_range: [0, 0]` (once per session, idempotent on repeat calls).
/// When `transcript_path` is `Some` but the file is absent, returns `None`
/// (no-op: nothing to record).
pub fn capture_tail(
    state_root: &Path,
    session_id: &str,
    transcript_path: Option<&str>,
    capture_source: &str,
    custom_instructions: Option<&str>,
) -> anyhow::Result<Option<EpisodeRecord>> {
    let mut manifest = load_manifest(state_root)?;

    let watermark = manifest
        .consolidated_through_offsets
        .get(session_id)
        .copied()
        .unwrap_or(0);

    // Determine the byte range for this capture.
    let (start, end) = match transcript_path {
        Some(path) => {
            let file_len = match std::fs::metadata(path) {
                Ok(meta) => meta.len(),
                Err(_) => {
                    // File absent: no new bytes — no-op.
                    return Ok(None);
                }
            };
            // If the file was truncated/rotated below the prior watermark,
            // reset to capture from the beginning of the new file.
            let start = if file_len < watermark { 0 } else { watermark };
            if file_len <= start {
                // No new bytes — no-op.
                return Ok(None);
            }
            (start, file_len)
        }
        None => {
            // No transcript path; write a zero-range episode once (only if
            // watermark is still 0, to stay idempotent for repeated calls).
            if watermark > 0 {
                return Ok(None);
            }
            (0u64, 0u64)
        }
    };

    let mut episode = EpisodeRecord::new(
        session_id,
        capture_source,
        transcript_path.map(str::to_string),
        [start, end],
    );
    if let Some(ci) = custom_instructions {
        episode.custom_instructions = Some(ci.to_string());
    }

    // Write the episode file into <state-root>/episodic/.
    let dir = episodic_dir(state_root);
    std::fs::create_dir_all(&dir)?;
    let episode_path = dir.join(episode.filename());
    cortex_core::persist::write_atomic_json(&episode_path, &episode)
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    // Advance watermark and record the episode in the manifest.
    manifest
        .consolidated_through_offsets
        .insert(session_id.to_string(), end);
    manifest
        .episodes
        .insert(episode.episode_id.clone(), episode.clone());
    save_manifest(state_root, &manifest)?;

    Ok(Some(episode))
}
