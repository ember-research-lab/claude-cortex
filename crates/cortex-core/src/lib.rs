//! Cortex substrate (v3 native).
//!
//! v3 uses clean canonical JSON (serde default formatting) and RFC3339 `Z`
//! timestamps. v2 ledgers are NOT read directly — see `cortex-migrate` and
//! the [`v2_compat`] module for the one-shot conversion path.
//!
//! All hashes are SHA-256 (matching the v2 substrate; despite the v3 spec
//! mentioning BLAKE3, every other piece of cortex tooling expects SHA-256).

pub mod confidence;
pub mod error;
pub mod hashing;
pub mod merkle;
pub mod models;
pub mod objects;
pub mod signing;
pub mod store;
pub mod time;
pub mod v2_compat;

pub use confidence::{apply_outcome_delta, decay_confidence, ConfidenceConfig, OUTCOME_DELTAS};
pub use error::{Error, Result};
pub use hashing::{compute_block_hash, compute_content_hash};
pub use merkle::{MerkleNode, MerkleTree};
pub use models::{
    Block, BlockIndexEntry, GitSourceMetadata, Identity, Index, Learning, LearningCategory,
    LearningSource, Outcome, OutcomeResult, PrivacyLevel, ProjectContext, Reinforcement,
    ReinforcementOutcome, Reinforcements, TrustLevel, TrustedKey,
};
pub use objects::{ObjectStore, StoredLearning};
pub use signing::{KeyManager, KeyPair, SignatureCheck};
pub use store::Ledger;
pub use time::{format_rfc3339_z, parse_rfc3339, UtcTime};
