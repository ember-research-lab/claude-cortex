//! Embedding generation + content-addressed cache for the cortex spectral
//! layer (v4).
//!
//! Status: SCAFFOLD — types and traits only, no live implementation. The
//! Anthropic-API-backed provider is stubbed; the cache layer is stubbed; the
//! local-model fallback is deferred.
//!
//! ## Design decisions baked in (per v4 spec defaults + small-ledger context)
//!
//! - **Default provider**: Anthropic API. Better embedding quality than
//!   small open-source models; cost is amortized by the content-addressed
//!   cache (we never re-embed unchanged content).
//! - **Cache layout**: `<ledger-root>/embeddings/<first-2>/<full-32-hash>.json`,
//!   sharded the same way as cortex-core's object store. The hash is the
//!   SHA-256 of (content || provider-id || model-id) — change provider or
//!   model, get a fresh embedding; identical content + provider, hit cache.
//! - **Vector format**: f32 to match every common embedding API. Stored as
//!   a JSON array; binary format (npy / safetensors) deferred until a
//!   measured perf problem exists.
//! - **Async**: tokio. The provider trait is async because every real provider
//!   is network-bound.
//!
//! ## Surface
//!
//! - [`EmbeddingProvider`] — async trait, one impl per backend.
//! - [`AnthropicEmbeddings`] — Anthropic API client (TODO: implement).
//! - [`EmbeddingCache`] — sharded content-addressed store of `(meta, vector)` pairs.
//! - [`Embedder`] — combines a provider + cache into the surface other v4
//!   crates use; calls `cache.get_or_insert(content, || provider.embed(content))`.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// A single embedding vector with provenance metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Embedding {
    pub vector: Vec<f32>,
    pub meta: EmbeddingMeta,
}

/// Provenance for an embedding so the cache can detect provider/model drift.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmbeddingMeta {
    /// Stable provider identifier, e.g. `"anthropic"` or `"local-bge-large"`.
    pub provider: String,
    /// Model identifier within the provider, e.g. `"claude-embed-1"`.
    pub model: String,
    /// Vector dimensionality. Stored so consumers can validate before use.
    pub dim: u32,
}

#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    fn meta(&self) -> EmbeddingMeta;
    async fn embed(&self, content: &str) -> anyhow::Result<Embedding>;

    /// Default impl iterates serially. Providers that batch should override.
    async fn embed_many(&self, contents: &[String]) -> anyhow::Result<Vec<Embedding>> {
        let mut out = Vec::with_capacity(contents.len());
        for c in contents {
            out.push(self.embed(c).await?);
        }
        Ok(out)
    }
}

/// Anthropic-hosted embedding provider. **Stub** — no network calls yet.
/// When implemented, will hit the embedding endpoint Aaron's lab uses, with
/// retries on 429/5xx and an explicit timeout per request.
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
            dim: 0, // populated after first call once we know the API output dim
        }
    }

    async fn embed(&self, _content: &str) -> anyhow::Result<Embedding> {
        anyhow::bail!("AnthropicEmbeddings::embed: not yet implemented (v4 Phase 1 stub)")
    }
}

/// Sharded content-addressed embedding cache. **Stub** — no on-disk impl yet.
pub struct EmbeddingCache {
    pub root: PathBuf,
}

impl EmbeddingCache {
    pub fn open(root: impl Into<PathBuf>) -> anyhow::Result<Self> {
        let root = root.into();
        std::fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    /// Returns the on-disk path an embedding would live at.
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

/// Provider + cache combined. The surface other v4 crates use.
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

/// Resolve the embedding cache root for a given ledger.
/// Layout: `<ledger-root>/embeddings/`.
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
