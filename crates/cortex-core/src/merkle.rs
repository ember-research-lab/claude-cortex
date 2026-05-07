//! Binary Merkle tree over (block_id, block_hash) leaves.
//!
//! Hash construction (matches v2 wire format byte-for-byte so cortex-migrate
//! can replay v2 trees during transcription):
//! - Leaf hash:  `SHA256("leaf:{block_id}:{block_hash}")` hex.
//! - Pair hash:  `SHA256("{left_hash}:{right_hash}")` hex.
//! - Odd levels: pad with `SHA256("")` hex (`EMPTY_HASH`).
//! - Leaves are sorted by `block_id` for deterministic trees.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;

use crate::error::{Error, Result};

const MERKLE_VERSION: u32 = 1;

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn empty_hash() -> String {
    sha256_hex(b"")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MerkleNode {
    pub hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left: Option<Box<MerkleNode>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub right: Option<Box<MerkleNode>>,
}

impl MerkleNode {
    pub fn is_leaf(&self) -> bool {
        self.block_id.is_some()
    }

    fn leaf(block_id: String, block_hash: String) -> Self {
        let hash = Self::hash_leaf(&block_id, &block_hash);
        Self {
            hash,
            block_id: Some(block_id),
            block_hash: Some(block_hash),
            left: None,
            right: None,
        }
    }

    pub fn hash_leaf(block_id: &str, block_hash: &str) -> String {
        sha256_hex(format!("leaf:{block_id}:{block_hash}").as_bytes())
    }

    pub fn hash_pair(left: &str, right: &str) -> String {
        sha256_hex(format!("{left}:{right}").as_bytes())
    }
}

#[derive(Debug, Clone, Default)]
pub struct MerkleTree {
    root: Option<MerkleNode>,
    leaf_count: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct MerkleFile {
    version: u32,
    leaf_count: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    root_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    root: Option<MerkleNode>,
}

impl MerkleTree {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn build<I>(leaves: I) -> Self
    where
        I: IntoIterator<Item = (String, String)>,
    {
        let mut leaves: Vec<(String, String)> = leaves.into_iter().collect();
        leaves.sort_by(|a, b| a.0.cmp(&b.0));
        let leaf_count = leaves.len() as u64;
        if leaves.is_empty() {
            return Self {
                root: None,
                leaf_count: 0,
            };
        }
        let mut nodes: Vec<MerkleNode> = leaves
            .into_iter()
            .map(|(id, h)| MerkleNode::leaf(id, h))
            .collect();
        while nodes.len() > 1 {
            let mut next = Vec::with_capacity(nodes.len().div_ceil(2));
            let mut iter = nodes.into_iter();
            while let Some(left) = iter.next() {
                let right = iter.next().unwrap_or(MerkleNode {
                    hash: empty_hash(),
                    block_id: None,
                    block_hash: None,
                    left: None,
                    right: None,
                });
                let parent_hash = MerkleNode::hash_pair(&left.hash, &right.hash);
                next.push(MerkleNode {
                    hash: parent_hash,
                    block_id: None,
                    block_hash: None,
                    left: Some(Box::new(left)),
                    right: Some(Box::new(right)),
                });
            }
            nodes = next;
        }
        Self {
            root: Some(nodes.into_iter().next().unwrap()),
            leaf_count,
        }
    }

    pub fn root_hash(&self) -> Option<&str> {
        self.root.as_ref().map(|r| r.hash.as_str())
    }

    pub fn leaf_count(&self) -> u64 {
        self.leaf_count
    }

    pub fn root(&self) -> Option<&MerkleNode> {
        self.root.as_ref()
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let file = MerkleFile {
            version: MERKLE_VERSION,
            leaf_count: self.leaf_count,
            root_hash: self.root.as_ref().map(|r| r.hash.clone()),
            root: self.root.clone(),
        };
        crate::objects::write_atomic_json(path, &file)
    }

    pub fn load(path: &Path) -> Result<Option<Self>> {
        if !path.is_file() {
            return Ok(None);
        }
        let bytes = std::fs::read(path).map_err(|e| Error::io(path, e))?;
        let file: MerkleFile = serde_json::from_slice(&bytes).map_err(|e| Error::json(path, e))?;
        if file.version != MERKLE_VERSION {
            return Err(Error::Malformed(format!(
                "merkle.json version {} (expected {})",
                file.version, MERKLE_VERSION
            )));
        }
        Ok(Some(Self {
            root: file.root,
            leaf_count: file.leaf_count,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_tree_has_no_root() {
        let tree = MerkleTree::build(std::iter::empty::<(String, String)>());
        assert!(tree.root_hash().is_none());
        assert_eq!(tree.leaf_count(), 0);
    }

    #[test]
    fn root_hash_is_deterministic_under_input_order() {
        let leaves_a = vec![
            ("b".to_string(), "h1".to_string()),
            ("a".to_string(), "h2".to_string()),
        ];
        let leaves_b = vec![
            ("a".to_string(), "h2".to_string()),
            ("b".to_string(), "h1".to_string()),
        ];
        assert_eq!(
            MerkleTree::build(leaves_a).root_hash().map(str::to_string),
            MerkleTree::build(leaves_b).root_hash().map(str::to_string)
        );
    }

    #[test]
    fn matches_known_v2_root() {
        // Real values pulled from tests/fixtures/v2_ledger/merkle.json (block IDs are
        // sorted alphabetically by the v2 builder, which we match exactly).
        let leaves = vec![
            (
                "e58f7d8e-3d37-471d-96b5-1c7a38cb7ca8".to_string(),
                "093f7b138873aae51e5cfa02c9a9c31a4886d70949f7470763fce09d7303ae27".to_string(),
            ),
            (
                "ea3f642c-cb9d-4563-9dea-a618d425cb59".to_string(),
                "b9d40b365984a66e0fdd85081d7c06ee759f48c23b9194f32ec8564fd8f19c72".to_string(),
            ),
        ];
        let tree = MerkleTree::build(leaves);
        assert_eq!(
            tree.root_hash(),
            Some("bab6c3698671a14dc6a7fbc41032b8cf3bc5738a74326ccd1a56202b38404a36"),
        );
    }
}
