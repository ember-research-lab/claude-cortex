//! Episodic memory substrate for cortex.
//!
//! Provides types and functions for capturing, storing, consolidating, and
//! evicting episode records — short-lived transcript slices that bridge
//! individual Claude Code sessions until their learnings are promoted into
//! the durable long-term ledger.

pub mod capture;
pub mod episode;
pub mod eviction;
pub mod manifest;

pub use capture::capture_tail;
pub use episode::{EpisodeRecord, EpisodeStatus};
pub use eviction::{prune_evictable, reconcile_eviction};
pub use manifest::{episodic_dir, load_manifest, manifest_path, save_manifest, EpisodeManifest};
