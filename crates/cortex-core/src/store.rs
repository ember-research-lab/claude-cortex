//! v3 ledger: index.json, blocks/, reinforcements.json, merkle.json.
//!
//! Concurrent access uses `fs2` advisory file locks on the relevant index
//! file. Block writes are atomic (temp + rename) and the index is updated
//! within the same lock so a crash never leaves an orphan block.

use std::path::{Path, PathBuf};
use std::time::Duration;

use fs2::FileExt;
use serde::Serialize;

use crate::error::{Error, Result};
use crate::hashing::compute_block_hash;
use crate::merkle::MerkleTree;
use crate::models::{
    Block, BlockIndexEntry, Index, Learning, OutcomeResult, Reinforcement, ReinforcementOutcome,
    Reinforcements,
};
use crate::objects::{write_atomic_json, ObjectStore};
use crate::signing::KeyManager;
use crate::time::UtcTime;

const LOCK_RETRY_MS: u64 = 25;
const LOCK_TIMEOUT_MS: u64 = 5_000;

pub struct Ledger {
    pub root: PathBuf,
}

impl Ledger {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        std::fs::create_dir_all(&root).map_err(|e| Error::io(&root, e))?;
        let ledger = Self { root };
        ledger.ensure_layout()?;
        Ok(ledger)
    }

    fn ensure_layout(&self) -> Result<()> {
        let blocks = self.blocks_dir();
        std::fs::create_dir_all(&blocks).map_err(|e| Error::io(&blocks, e))?;
        if !self.index_path().is_file() {
            write_atomic_json(&self.index_path(), &Index::default())?;
        }
        if !self.reinforcements_path().is_file() {
            write_atomic_json(&self.reinforcements_path(), &Reinforcements::default())?;
        }
        Ok(())
    }

    pub fn index_path(&self) -> PathBuf {
        self.root.join("index.json")
    }

    pub fn reinforcements_path(&self) -> PathBuf {
        self.root.join("reinforcements.json")
    }

    pub fn merkle_path(&self) -> PathBuf {
        self.root.join("merkle.json")
    }

    pub fn blocks_dir(&self) -> PathBuf {
        self.root.join("blocks")
    }

    pub fn block_path(&self, block_id: &str) -> PathBuf {
        self.blocks_dir().join(format!("{block_id}.json"))
    }

    pub fn key_manager(&self) -> KeyManager {
        KeyManager::new(&self.root)
    }

    pub fn object_store(&self) -> Result<ObjectStore> {
        ObjectStore::open(&self.root)
    }

    pub fn read_index(&self) -> Result<Index> {
        let path = self.index_path();
        let bytes = std::fs::read(&path).map_err(|e| Error::io(&path, e))?;
        serde_json::from_slice(&bytes).map_err(|e| Error::json(&path, e))
    }

    pub fn read_reinforcements(&self) -> Result<Reinforcements> {
        let path = self.reinforcements_path();
        let bytes = std::fs::read(&path).map_err(|e| Error::io(&path, e))?;
        serde_json::from_slice(&bytes).map_err(|e| Error::json(&path, e))
    }

    pub fn read_block(&self, block_id: &str) -> Result<Option<Block>> {
        let path = self.block_path(block_id);
        if !path.is_file() {
            return Ok(None);
        }
        let bytes = std::fs::read(&path).map_err(|e| Error::io(&path, e))?;
        let block: Block = serde_json::from_slice(&bytes).map_err(|e| Error::json(&path, e))?;
        Ok(Some(block))
    }

    pub fn head(&self) -> Result<Option<String>> {
        Ok(self.read_index()?.head)
    }

    pub fn append_block(
        &self,
        session_id: impl Into<String>,
        learnings: Vec<Learning>,
        sign: bool,
    ) -> Result<Block> {
        let session_id = session_id.into();
        let _guard = exclusive_lock(&self.index_path())?;

        let mut index = self.read_index()?;
        let parent = index.head.clone();
        let now = UtcTime::now();
        let id = uuid::Uuid::new_v4().to_string();

        let mut block = Block {
            id: id.clone(),
            timestamp: now,
            session_id,
            parent_block: parent.clone(),
            learnings: learnings.clone(),
            author_key_id: None,
            signature: None,
            hash: String::new(),
        };
        block.hash = compute_block_hash(&block);

        if sign {
            if let Some((key_id, signature)) = self.key_manager().sign_block_hash(&block.hash)? {
                block.author_key_id = Some(key_id);
                block.signature = Some(signature);
            }
        }

        let block_path = self.block_path(&id);
        write_atomic_json(&block_path, &block)?;

        index.head = Some(id.clone());
        index.blocks.push(BlockIndexEntry {
            id: id.clone(),
            timestamp: now,
            hash: block.hash.clone(),
            parent,
        });

        let merkle = self.update_merkle_tree(&index)?;
        index.merkle_root = merkle.root_hash().map(str::to_string);
        write_atomic_json(&self.index_path(), &index)?;

        if !learnings.is_empty() {
            self.register_learnings(&id, &learnings)?;
        }

        Ok(block)
    }

    fn update_merkle_tree(&self, index: &Index) -> Result<MerkleTree> {
        let leaves = index.blocks.iter().map(|b| (b.id.clone(), b.hash.clone()));
        let tree = MerkleTree::build(leaves);
        tree.save(&self.merkle_path())?;
        Ok(tree)
    }

    fn register_learnings(&self, block_id: &str, learnings: &[Learning]) -> Result<()> {
        let store = self.object_store()?;
        let mut reinforcements = self.read_reinforcements()?;
        for learning in learnings {
            let object_hash = store.store_learning(learning)?;
            let now = UtcTime::now();
            reinforcements.learnings.insert(
                learning.id.clone(),
                Reinforcement {
                    category: learning.category,
                    content: learning.content.clone(),
                    confidence: learning.confidence,
                    outcome_count: 0,
                    last_updated: now,
                    last_applied: now,
                    block_id: block_id.to_string(),
                    content_hash: learning.content_hash.clone(),
                    object_store_hash: object_hash,
                    outcomes: Vec::new(),
                },
            );
        }
        write_atomic_json(&self.reinforcements_path(), &reinforcements)?;
        Ok(())
    }

    pub fn record_outcome(
        &self,
        learning_id: &str,
        result: OutcomeResult,
        context: impl Into<String>,
    ) -> Result<f64> {
        let _guard = exclusive_lock(&self.reinforcements_path())?;
        let mut reinforcements = self.read_reinforcements()?;
        let reinforcement = reinforcements
            .learnings
            .get_mut(learning_id)
            .ok_or_else(|| {
                Error::Malformed(format!(
                    "learning {learning_id} not found in reinforcements"
                ))
            })?;
        let delta = crate::confidence::delta_for(result);
        let new_confidence =
            crate::confidence::apply_outcome_delta(reinforcement.confidence, result);
        let now = UtcTime::now();
        reinforcement.outcomes.push(ReinforcementOutcome {
            timestamp: now,
            result,
            context: context.into(),
            delta,
        });
        reinforcement.confidence = new_confidence;
        reinforcement.outcome_count += 1;
        reinforcement.last_updated = now;
        reinforcement.last_applied = now;
        let confidence = reinforcement.confidence;
        write_atomic_json(&self.reinforcements_path(), &reinforcements)?;
        Ok(confidence)
    }

    pub fn verify_chain(&self) -> Result<ChainReport> {
        let index = self.read_index()?;
        let key_manager = self.key_manager();
        let mut report = ChainReport::default();
        for entry in &index.blocks {
            let Some(block) = self.read_block(&entry.id)? else {
                report.missing_blocks.push(entry.id.clone());
                continue;
            };
            let recomputed = compute_block_hash(&block);
            if recomputed != block.hash || recomputed != entry.hash {
                report.hash_mismatches.push(entry.id.clone());
                continue;
            }
            if let (Some(key_id), Some(signature)) =
                (block.author_key_id.as_ref(), block.signature.as_ref())
            {
                let check = key_manager.verify_block_signature(&block.hash, key_id, signature)?;
                if !matches!(check, crate::signing::SignatureCheck::Valid) {
                    report.signature_failures.push((entry.id.clone(), check));
                    continue;
                }
            }
            report.valid_blocks.push(entry.id.clone());
        }
        let computed_root =
            MerkleTree::build(index.blocks.iter().map(|b| (b.id.clone(), b.hash.clone())))
                .root_hash()
                .map(str::to_string);
        if computed_root != index.merkle_root {
            report.merkle_mismatch = Some((index.merkle_root.clone(), computed_root));
        }
        Ok(report)
    }
}

#[derive(Debug, Default)]
pub struct ChainReport {
    pub valid_blocks: Vec<String>,
    pub missing_blocks: Vec<String>,
    pub hash_mismatches: Vec<String>,
    pub signature_failures: Vec<(String, crate::signing::SignatureCheck)>,
    pub merkle_mismatch: Option<(Option<String>, Option<String>)>,
}

impl ChainReport {
    pub fn is_clean(&self) -> bool {
        self.missing_blocks.is_empty()
            && self.hash_mismatches.is_empty()
            && self.signature_failures.is_empty()
            && self.merkle_mismatch.is_none()
    }
}

struct LockGuard {
    file: std::fs::File,
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

fn exclusive_lock(path: &Path) -> Result<LockGuard> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(|e| Error::io(path, e))?;
    let deadline = std::time::Instant::now() + Duration::from_millis(LOCK_TIMEOUT_MS);
    loop {
        match file.try_lock_exclusive() {
            Ok(()) => return Ok(LockGuard { file }),
            Err(_) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(LOCK_RETRY_MS));
            }
            Err(e) => return Err(Error::io(path, e)),
        }
    }
}

#[doc(hidden)]
pub fn _write_atomic_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    write_atomic_json(path, value)
}
