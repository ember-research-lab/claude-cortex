//! # cortex-audit — signed, hash-chained **action**-audit ledger
//!
//! The tamper-evident log of every agentic *action* — distinct from the cortex
//! *learning* ledger (`cortex-core`). It exists because the v0.1 pressure-test of
//! the Ember SMB platform found three disqualifying properties in `cortex-core`'s
//! block signing for an **audit** use, and this crate is built specifically to NOT
//! have them:
//!
//! 1. **Records are ACTIONS, not learnings** — `{ principal, tenant, kind, detail,
//!    timestamp }`, with a mandatory acting **principal**.
//! 2. **The principal is bound INTO the hashed + signed payload** — the entry hash
//!    covers the principal, and the Ed25519 signature is over that hash, so
//!    authorship is cryptographically bound (cortex-core excludes `author_key_id`
//!    from its hash).
//! 3. **An unsigned entry FAILS verification** — `verify()` returns
//!    [`ChainStatus::Unsigned`], never `Clean` (cortex-core verifies unsigned
//!    blocks clean).
//!
//! Append-only, hash-chained (`prev_hash = sha256(prev_entry)`), Ed25519-signed.
//! Multi-consumer: the platforms consume it behind their own `ActionLedger` trait.

#![forbid(unsafe_code)]

use ed25519_dalek::{Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

/// What an agent did. The `principal` is the acting identity (e.g. `agent:acme:reception`,
/// `human:alice`, `tenant:acme`) — mandatory and bound into the signed hash.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionRecord {
    pub principal: String,
    pub tenant: String,
    pub kind: String,
    pub detail: String,
    pub timestamp_ms: u64,
}

/// A signed, hash-chained ledger entry. `hash` covers `(seq, prev_hash, record)`
/// — the record *includes* the principal — and `signature` is over `hash`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SignedEntry {
    pub seq: u64,
    pub prev_hash: String,
    pub hash: String,
    /// Hex Ed25519 public key id of the signer. Empty ⇒ unsigned ⇒ verification fails.
    pub key_id: String,
    /// Hex Ed25519 signature over `hash`. Empty ⇒ unsigned ⇒ verification fails.
    pub signature: String,
    pub record: ActionRecord,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChainStatus {
    Clean,
    /// Hash-chain discontinuity or a recomputed-hash mismatch (tamper) at `seq`.
    HashBreak(u64),
    /// Signature did not verify at `seq`.
    BadSignature(u64),
    /// Signer's key is not in the trusted set at `seq`.
    UntrustedKey(u64),
    /// An entry with no/empty signature — a FAILURE, never clean.
    Unsigned(u64),
}

#[derive(Debug, thiserror::Error)]
pub enum AuditError {
    #[error("the ledger has no signing key; an audit entry must be signed")]
    NoSigningKey,
}

const GENESIS: &str = "genesis";

/// Canonical bytes that the entry hash is computed over. Includes the principal
/// (inside `record`), so authorship is part of the hash — and thus the signature.
fn entry_hash(seq: u64, prev_hash: &str, record: &ActionRecord) -> String {
    // Deterministic: canonical JSON of the record + the chain position.
    let canonical = serde_json::to_string(record).expect("ActionRecord serializes");
    let mut h = Sha256::new();
    h.update(seq.to_le_bytes());
    h.update(prev_hash.as_bytes());
    h.update(b"\0");
    h.update(canonical.as_bytes());
    hex::encode(h.finalize())
}

fn key_id_of(vk: &VerifyingKey) -> String {
    hex::encode(vk.to_bytes())
}

/// The action-audit ledger. Holds the signing key, the hash-chained entries, and
/// the trusted public keys used at verification.
pub struct AuditLedger {
    signing: SigningKey,
    key_id: String,
    trusted: HashMap<String, VerifyingKey>,
    entries: Vec<SignedEntry>,
}

impl AuditLedger {
    /// Open a ledger that signs with `signing`. The signer's own key is trusted.
    pub fn new(signing: SigningKey) -> Self {
        let vk = signing.verifying_key();
        let key_id = key_id_of(&vk);
        let mut trusted = HashMap::new();
        trusted.insert(key_id.clone(), vk);
        Self {
            signing,
            key_id,
            trusted,
            entries: Vec::new(),
        }
    }

    /// Trust an additional public key (e.g. a key that signed older entries after rotation).
    pub fn trust(&mut self, vk: VerifyingKey) {
        self.trusted.insert(key_id_of(&vk), vk);
    }

    /// Append an action — signed, with the principal bound into the hash. An audit
    /// entry is **always** signed; there is no unsigned append.
    pub fn append(&mut self, record: ActionRecord) -> Result<&SignedEntry, AuditError> {
        let seq = self.entries.len() as u64;
        let prev_hash = self
            .entries
            .last()
            .map(|e| e.hash.clone())
            .unwrap_or_else(|| GENESIS.to_string());
        let hash = entry_hash(seq, &prev_hash, &record);
        let signature = self.signing.sign(hash.as_bytes());
        self.entries.push(SignedEntry {
            seq,
            prev_hash,
            hash,
            key_id: self.key_id.clone(),
            signature: hex::encode(signature.to_bytes()),
            record,
        });
        Ok(self.entries.last().expect("just pushed"))
    }

    /// Verify the whole chain: hash continuity, recomputed-hash match (tamper),
    /// **mandatory** signature, and signature validity against a trusted key.
    pub fn verify(&self) -> ChainStatus {
        let mut prev = GENESIS.to_string();
        for e in &self.entries {
            if e.prev_hash != prev {
                return ChainStatus::HashBreak(e.seq);
            }
            // Recompute the hash from the record (incl. principal) — catches tamper.
            if entry_hash(e.seq, &e.prev_hash, &e.record) != e.hash {
                return ChainStatus::HashBreak(e.seq);
            }
            // An audit entry MUST be signed — unsigned is a failure, never clean.
            if e.signature.is_empty() || e.key_id.is_empty() {
                return ChainStatus::Unsigned(e.seq);
            }
            let Some(vk) = self.trusted.get(&e.key_id) else {
                return ChainStatus::UntrustedKey(e.seq);
            };
            let sig_bytes = match hex::decode(&e.signature)
                .ok()
                .and_then(|b| <[u8; 64]>::try_from(b).ok())
            {
                Some(b) => b,
                None => return ChainStatus::BadSignature(e.seq),
            };
            let sig = ed25519_dalek::Signature::from_bytes(&sig_bytes);
            if vk.verify(e.hash.as_bytes(), &sig).is_err() {
                return ChainStatus::BadSignature(e.seq);
            }
            prev = e.hash.clone();
        }
        ChainStatus::Clean
    }

    pub fn entries(&self) -> &[SignedEntry] {
        &self.entries
    }

    /// Entries for a tenant (the platform's board view).
    pub fn entries_for_tenant<'a>(&'a self, tenant: &str) -> impl Iterator<Item = &'a SignedEntry> {
        let tenant = tenant.to_string();
        self.entries
            .iter()
            .filter(move |e| e.record.tenant == tenant)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::OsRng;

    fn ledger() -> AuditLedger {
        AuditLedger::new(SigningKey::generate(&mut OsRng))
    }

    fn rec(principal: &str, tenant: &str, kind: &str) -> ActionRecord {
        ActionRecord {
            principal: principal.into(),
            tenant: tenant.into(),
            kind: kind.into(),
            detail: "d".into(),
            timestamp_ms: 0,
        }
    }

    #[test]
    fn append_signs_and_chain_verifies_clean() {
        let mut l = ledger();
        l.append(rec("agent:a1", "t1", "send_email")).unwrap();
        l.append(rec("agent:a1", "t1", "log_note")).unwrap();
        assert_eq!(l.verify(), ChainStatus::Clean);
        assert_eq!(l.entries().len(), 2);
        // each entry is signed
        assert!(l.entries().iter().all(|e| !e.signature.is_empty()));
    }

    #[test]
    fn unsigned_entry_fails_verification() {
        // The cortex-core bug this crate must not have: an unsigned entry must NOT
        // verify clean.
        let mut l = ledger();
        l.append(rec("agent:a1", "t1", "act")).unwrap();
        // strip the signature on the entry (simulate an unsigned/forged-clean entry)
        l.entries[0].signature.clear();
        assert!(matches!(l.verify(), ChainStatus::Unsigned(0)));
    }

    #[test]
    fn tampering_the_principal_breaks_the_hash() {
        // Authorship is bound into the hash: changing the principal without re-hashing
        // is caught (the cortex-core bug was that author_key_id was outside the hash).
        let mut l = ledger();
        l.append(rec("agent:a1", "t1", "act")).unwrap();
        assert_eq!(l.verify(), ChainStatus::Clean);
        l.entries[0].record.principal = "agent:attacker".into();
        assert!(matches!(l.verify(), ChainStatus::HashBreak(0)));
    }

    #[test]
    fn tampering_a_signed_field_is_caught() {
        let mut l = ledger();
        l.append(rec("agent:a1", "t1", "act")).unwrap();
        l.entries[0].record.detail = "changed".into();
        assert!(matches!(l.verify(), ChainStatus::HashBreak(0)));
    }

    #[test]
    fn untrusted_signer_is_rejected() {
        let mut l = ledger();
        l.append(rec("agent:a1", "t1", "act")).unwrap();
        // re-sign with a different key whose id we don't trust
        let rogue = SigningKey::generate(&mut OsRng);
        let e = &mut l.entries[0];
        e.key_id = hex::encode(rogue.verifying_key().to_bytes());
        e.signature = hex::encode(rogue.sign(e.hash.as_bytes()).to_bytes());
        assert!(matches!(l.verify(), ChainStatus::UntrustedKey(0)));
    }

    #[test]
    fn tenant_view_filters() {
        let mut l = ledger();
        l.append(rec("agent:a1", "t1", "act")).unwrap();
        l.append(rec("agent:a2", "t2", "act")).unwrap();
        assert_eq!(l.entries_for_tenant("t1").count(), 1);
        assert_eq!(l.entries_for_tenant("t2").count(), 1);
    }
}
