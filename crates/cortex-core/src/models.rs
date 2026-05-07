//! Core data models for the v3 ledger.
//!
//! Field names and shapes are kept compatible with v2 where possible, but the
//! v3 wire format uses standard serde_json + RFC3339 `Z` timestamps. v2 →
//! v3 conversion is handled by `cortex-migrate`, not by these types directly.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::time::UtcTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LearningCategory {
    Discovery,
    Decision,
    Error,
    Pattern,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PrivacyLevel {
    #[default]
    Public,
    Project,
    Private,
    Redacted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutcomeResult {
    Success,
    Failure,
    Partial,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LearningSource {
    #[default]
    Session,
    GitCommit,
    GitDiff,
    PrDescription,
    PrReview,
    PrComment,
    Manual,
    Import,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TrustLevel {
    Full,
    Marginal,
    None,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    pub name: String,
    pub machine: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustedKey {
    pub key_id: String,
    /// Base64-encoded raw 32-byte Ed25519 public key.
    pub public_key: String,
    pub identity: Identity,
    pub trust_level: TrustLevel,
    pub added_at: UtcTime,
    #[serde(default)]
    pub vouched_by: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProjectContext {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_type: Option<String>,
    #[serde(default)]
    pub tech_stack: Vec<String>,
    #[serde(default)]
    pub keywords: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GitSourceMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit_sha: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit_short_sha: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit_author_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit_author_email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit_date: Option<UtcTime>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit_subject: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_number: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_author: Option<String>,
}

/// Outcome attached to a learning, recorded in `reinforcements.json`.
/// The `delta` field is informational only — confidence is recomputed from
/// the canonical [`crate::confidence`] table on read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Outcome {
    pub timestamp: UtcTime,
    pub result: OutcomeResult,
    pub context: String,
    pub delta: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Learning {
    pub id: String,
    pub category: LearningCategory,
    pub content: String,
    pub content_hash: String,
    pub confidence: f64,
    #[serde(default)]
    pub privacy: PrivacyLevel,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub created_at: UtcTime,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_applied: Option<UtcTime>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_context: Option<ProjectContext>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub derived_from: Option<String>,
    #[serde(default)]
    pub learning_source: LearningSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_metadata: Option<GitSourceMetadata>,
    #[serde(default)]
    pub co_authors: Vec<String>,
}

impl Learning {
    pub fn new(
        category: LearningCategory,
        content: impl Into<String>,
        confidence: f64,
        source: Option<String>,
    ) -> Self {
        let content = content.into();
        let hash = crate::hashing::compute_content_hash(&content);
        Self {
            id: Uuid::new_v4().to_string(),
            category,
            content,
            content_hash: hash,
            confidence: confidence.clamp(0.0, 1.0),
            privacy: PrivacyLevel::default(),
            source,
            created_at: UtcTime::now(),
            last_applied: None,
            project_context: None,
            derived_from: None,
            learning_source: LearningSource::default(),
            git_metadata: None,
            co_authors: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Block {
    pub id: String,
    pub timestamp: UtcTime,
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_block: Option<String>,
    #[serde(default)]
    pub learnings: Vec<Learning>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_key_id: Option<String>,
    /// Base64 Ed25519 signature of the canonical [`crate::compute_block_hash`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    /// SHA-256 hex of the canonical block content. Recomputable from the
    /// other fields; stored on disk for fast reads.
    pub hash: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BlockIndexEntry {
    pub id: String,
    pub timestamp: UtcTime,
    pub hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Index {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
    #[serde(default)]
    pub blocks: Vec<BlockIndexEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merkle_root: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Reinforcements {
    #[serde(default)]
    pub learnings: std::collections::BTreeMap<String, Reinforcement>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reinforcement {
    pub category: LearningCategory,
    pub content: String,
    pub confidence: f64,
    pub outcome_count: u64,
    pub last_updated: UtcTime,
    pub last_applied: UtcTime,
    pub block_id: String,
    pub content_hash: String,
    pub object_store_hash: String,
    #[serde(default)]
    pub outcomes: Vec<ReinforcementOutcome>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReinforcementOutcome {
    pub timestamp: UtcTime,
    pub result: OutcomeResult,
    pub context: String,
    pub delta: f64,
}
