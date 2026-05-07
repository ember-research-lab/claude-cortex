//! Embedding generation + content-addressed cache for the cortex spectral
//! layer (v4).
//!
//! Status: SCAFFOLD — types and traits only, no live implementation. The
//! Anthropic-API-backed provider is stubbed; the cache layer is stubbed.
//!
//! ## Design decisions baked in
//!
//! - **Default provider**: Anthropic API. Better embedding quality than
//!   small open-source models; cost amortized by the content-addressed
//!   cache (we never re-embed unchanged content).
//! - **Cache layout**: `<ledger-root>/embeddings/<first-2>/<full-32-hash>.json`,
//!   sharded the same way as cortex-core's object store. The hash is the
//!   SHA-256 of (content || provider || model) — change provider or model,
//!   get a fresh embedding; identical content + provider, hit cache.
//! - **Vector format**: f32 to match every common embedding API. Stored as
//!   a JSON array; binary format deferred until measured perf problem.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Embedding {
    pub vector: Vec<f32>,
    pub meta: EmbeddingMeta,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmbeddingMeta {
    pub provider: String,
    pub model: String,
    pub dim: u32,
}

#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    fn meta(&self) -> EmbeddingMeta;
    async fn embed(&self, content: &str) -> anyhow::Result<Embedding>;
    async fn embed_many(&self, contents: &[String]) -> anyhow::Result<Vec<Embedding>> {
        let mut out = Vec::with_capacity(contents.len());
        for c in contents {
            out.push(self.embed(c).await?);
        }
        Ok(out)
    }
}

pub struct AnthropicEmbeddings {
    pub api_key: String,
    pub model: String,
    pub timeout: std::time::Duration,
}

impl AnthropicEmbeddings {
    pub fn new(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            model: model.into(),
            timeout: std::time::Duration::from_secs(30),
        }
    }
}

#[async_trait]
impl EmbeddingProvider for AnthropicEmbeddings {
    fn meta(&self) -> EmbeddingMeta {
        EmbeddingMeta {
            provider: "anthropic".to_string(),
            model: self.model.clone(),
            dim: 0,
        }
    }
    async fn embed(&self, _content: &str) -> anyhow::Result<Embedding> {
        anyhow::bail!("AnthropicEmbeddings::embed: not yet implemented (v4 Phase 1 stub)")
    }
}

pub struct EmbeddingCache {
    pub root: PathBuf,
}

impl EmbeddingCache {
    pub fn open(root: impl Into<PathBuf>) -> anyhow::Result<Self> {
        let root = root.into();
        std::fs::create_dir_all(&root)?;
        Ok(Self { root })
    }
    pub fn path_for(&self, content: &str, meta: &EmbeddingMeta) -> PathBuf {
        let key = cache_key(content, meta);
        let prefix = &key[..2];
        self.root.join(prefix).join(format!("{key}.json"))
    }
    pub fn get(&self, _content: &str, _meta: &EmbeddingMeta) -> anyhow::Result<Option<Embedding>> {
        anyhow::bail!("EmbeddingCache::get: not yet implemented (v4 Phase 1 stub)")
    }
    pub fn put(&self, _content: &str, _embedding: &Embedding) -> anyhow::Result<()> {
        anyhow::bail!("EmbeddingCache::put: not yet implemented (v4 Phase 1 stub)")
    }
}

fn cache_key(content: &str, meta: &EmbeddingMeta) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    hasher.update(b"||");
    hasher.update(meta.provider.as_bytes());
    hasher.update(b"||");
    hasher.update(meta.model.as_bytes());
    hex::encode(hasher.finalize())
}

pub struct Embedder<P: EmbeddingProvider> {
    pub provider: P,
    pub cache: EmbeddingCache,
}

impl<P: EmbeddingProvider> Embedder<P> {
    pub fn new(provider: P, cache: EmbeddingCache) -> Self {
        Self { provider, cache }
    }
    pub async fn embed_cached(&self, content: &str) -> anyhow::Result<Embedding> {
        let meta = self.provider.meta();
        if let Some(hit) = self.cache.get(content, &meta).ok().flatten() {
            return Ok(hit);
        }
        let fresh = self.provider.embed(content).await?;
        let _ = self.cache.put(content, &fresh);
        Ok(fresh)
    }
}

pub fn embedding_cache_root(ledger_root: &Path) -> PathBuf {
    ledger_root.join("embeddings")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_key_changes_when_provider_changes() {
        let m1 = EmbeddingMeta {
            provider: "anthropic".into(),
            model: "claude-embed-1".into(),
            dim: 1024,
        };
        let m2 = EmbeddingMeta {
            provider: "local-bge-large".into(),
            ..m1.clone()
        };
        assert_ne!(cache_key("hello", &m1), cache_key("hello", &m2));
    }

    #[test]
    fn cache_key_is_stable() {
        let m = EmbeddingMeta {
            provider: "anthropic".into(),
            model: "claude-embed-1".into(),
            dim: 1024,
        };
        assert_eq!(cache_key("hello", &m), cache_key("hello", &m));
    }
}
