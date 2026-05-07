//! Content and block hashing.
//!
//! v3 native hashing uses SHA-256 with canonical JSON for block hashes.
//! Canonical block JSON is `serde_json::to_string` over a sorted-key map of
//! the hash-relevant fields (no whitespace, RFC3339 `Z` timestamps).

use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::models::{Block, Learning};
use crate::time::format_rfc3339_z;

/// First 16 hex chars of SHA-256 over normalized content.
/// Normalization: lowercase, trim, collapse internal whitespace to single spaces.
/// Matches v2 byte-for-byte so existing object-store hashes round-trip.
pub fn compute_content_hash(content: &str) -> String {
    let normalized = normalize_content(content);
    let digest = Sha256::digest(normalized.as_bytes());
    hex_lower(&digest)[..16].to_string()
}

fn normalize_content(s: &str) -> String {
    let lower = s.to_lowercase();
    let trimmed = lower.trim();
    let mut out = String::with_capacity(trimmed.len());
    let mut last_was_ws = false;
    for c in trimmed.chars() {
        if c.is_whitespace() {
            if !last_was_ws {
                out.push(' ');
            }
            last_was_ws = true;
        } else {
            out.push(c);
            last_was_ws = false;
        }
    }
    out
}

/// SHA-256 hex of the canonical block content. Sorted-key JSON, no whitespace,
/// RFC3339 `Z` timestamps, only the hash-relevant fields.
pub fn compute_block_hash(block: &Block) -> String {
    let canonical = canonical_block_value(block);
    let bytes = serde_json::to_vec(&canonical).expect("canonical block hash JSON must serialize");
    hex_lower(&Sha256::digest(&bytes))
}

fn canonical_block_value(block: &Block) -> Value {
    let mut top = Map::new();
    top.insert("id".to_string(), Value::String(block.id.clone()));
    top.insert(
        "timestamp".to_string(),
        Value::String(format_rfc3339_z(&block.timestamp.into_inner())),
    );
    top.insert(
        "session_id".to_string(),
        Value::String(block.session_id.clone()),
    );
    top.insert(
        "parent_block".to_string(),
        match &block.parent_block {
            Some(p) => Value::String(p.clone()),
            None => Value::Null,
        },
    );
    let learnings: Vec<Value> = block
        .learnings
        .iter()
        .map(canonical_learning_value)
        .collect();
    top.insert("learnings".to_string(), Value::Array(learnings));
    Value::Object(top)
}

fn canonical_learning_value(learning: &Learning) -> Value {
    json!({
        "id": learning.id,
        "category": learning.category,
        "content": learning.content,
        "confidence": learning.confidence,
        "source": learning.source,
        "content_hash": learning.content_hash,
        "created_at": format_rfc3339_z(&learning.created_at.into_inner()),
    })
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write;
        write!(s, "{b:02x}").unwrap();
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_hash_matches_v2_normalization() {
        assert_eq!(
            compute_content_hash("cortex-core preserves v2 substrate format byte for byte"),
            "6d3ff6f085b139b5"
        );
        assert_eq!(
            compute_content_hash("atomic writes use tempfile + rename inside a flock-held parent"),
            "119165dfdc5def6e"
        );
    }

    #[test]
    fn content_hash_is_whitespace_insensitive() {
        let a = compute_content_hash("foo bar baz");
        let b = compute_content_hash("  FOO   bar\nbaz  ");
        assert_eq!(a, b);
    }
}
