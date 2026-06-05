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
//!
//! ## Truncation / rollback — the consumer MUST pin the head
//!
//! A hash chain alone cannot detect **truncation**: any prefix of a valid chain
//! is itself a valid chain, so an attacker who can rewrite the stored entries can
//! silently drop the tail (exactly the actions most worth hiding) and [`verify`]
//! (entries-only) still reports `Clean`. Consumers must therefore record the
//! [`AuditLedger::head`] `(seq, hash)` checkpoint **out-of-band** (somewhere the
//! ledger-file writer cannot reach) after appending, and verify with
//! [`AuditLedger::verify_at`] / [`verify_entries_at`], which additionally fail
//! with [`ChainStatus::HeadMismatch`] when the chain does not end at the pinned
//! head.
//!
//! [`verify`]: AuditLedger::verify

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
    /// The chain verified internally but does not end at the pinned head
    /// checkpoint — entries were truncated/rolled back, or the checkpoint
    /// belongs to a different fork. Carries the expected `(seq, hash)`.
    HeadMismatch {
        expected_seq: u64,
        expected_hash: String,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum AuditError {
    #[error("the ledger has no signing key; an audit entry must be signed")]
    NoSigningKey,
}

const GENESIS: &str = "genesis";

/// Canonical bytes that the entry hash is computed over. Includes the principal
/// (inside `record`), so authorship is part of the hash — and thus the signature.
///
/// Canonical form: `serde_json` of [`ActionRecord`], which emits fields in
/// **struct-declaration order** (`principal, tenant, kind, detail, timestamp_ms`).
/// That order is part of the wire format — reordering the struct fields, or
/// re-implementing this in another serializer, changes the bytes and breaks every
/// existing chain. The `hash_test_vector` test pins it; if you trip that test you
/// are changing the format, not fixing a bug.
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

/// Verify a chain of entries (e.g. reloaded from disk) against a trusted key set:
/// hash continuity, recomputed-hash match (tamper), **mandatory** signature, and
/// signature validity against a trusted key.
///
/// **Cannot detect truncation** — a valid prefix of a longer chain is `Clean`.
/// Use [`verify_entries_at`] with an out-of-band head checkpoint for that.
pub fn verify_entries(
    entries: &[SignedEntry],
    trusted: &HashMap<String, VerifyingKey>,
) -> ChainStatus {
    let mut prev = GENESIS.to_string();
    for e in entries {
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
        let Some(vk) = trusted.get(&e.key_id) else {
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

/// [`verify_entries`], plus the truncation/rollback check: the chain must end at
/// the pinned `(seq, hash)` head checkpoint the consumer recorded out-of-band.
pub fn verify_entries_at(
    entries: &[SignedEntry],
    trusted: &HashMap<String, VerifyingKey>,
    expected_head: (u64, &str),
) -> ChainStatus {
    let status = verify_entries(entries, trusted);
    if status != ChainStatus::Clean {
        return status;
    }
    let (expected_seq, expected_hash) = expected_head;
    match entries.last() {
        Some(last) if last.seq == expected_seq && last.hash == expected_hash => ChainStatus::Clean,
        _ => ChainStatus::HeadMismatch {
            expected_seq,
            expected_hash: expected_hash.to_string(),
        },
    }
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
    ///
    /// **Cannot detect truncation** — use [`AuditLedger::verify_at`] with the
    /// out-of-band head checkpoint (see the module docs).
    pub fn verify(&self) -> ChainStatus {
        verify_entries(&self.entries, &self.trusted)
    }

    /// [`AuditLedger::verify`], plus the truncation/rollback check against a
    /// pinned `(seq, hash)` head checkpoint (see the module docs).
    pub fn verify_at(&self, expected_head: (u64, &str)) -> ChainStatus {
        verify_entries_at(&self.entries, &self.trusted, expected_head)
    }

    /// The `(seq, hash)` head checkpoint after the last append. Consumers record
    /// this out-of-band and later verify with [`AuditLedger::verify_at`] —
    /// without it, truncation of the tail is undetectable. `None` when empty.
    pub fn head(&self) -> Option<(u64, &str)> {
        self.entries.last().map(|e| (e.seq, e.hash.as_str()))
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

    #[test]
    fn truncation_is_undetected_without_head_and_caught_with_it() {
        // The adversary: a writer who can rewrite the stored entries drops the
        // tail. The chain alone CANNOT catch this (any prefix is a valid chain) —
        // that is a documented limitation, pinned here. The pinned head catches it.
        let mut l = ledger();
        l.append(rec("agent:a1", "t1", "innocuous")).unwrap();
        l.append(rec("agent:a1", "t1", "innocuous")).unwrap();
        l.append(rec("agent:a1", "t1", "the_action_worth_hiding"))
            .unwrap();
        let (head_seq, head_hash) = {
            let (s, h) = l.head().unwrap();
            (s, h.to_string())
        };
        assert_eq!(l.verify_at((head_seq, &head_hash)), ChainStatus::Clean);

        // roll back the tail
        l.entries.pop();
        // entries-only verification is blind to it — documented, load-bearing:
        assert_eq!(l.verify(), ChainStatus::Clean);
        // the pinned head is not:
        assert_eq!(
            l.verify_at((head_seq, &head_hash)),
            ChainStatus::HeadMismatch {
                expected_seq: head_seq,
                expected_hash: head_hash.clone(),
            }
        );
        // ...including when truncated to empty
        l.entries.clear();
        assert!(matches!(
            l.verify_at((head_seq, &head_hash)),
            ChainStatus::HeadMismatch { .. }
        ));
    }

    #[test]
    fn mid_chain_forgery_with_full_rechain_is_caught() {
        // The adversary rewrites the file: inserts a forged entry mid-chain and
        // recomputes every downstream hash so the chain is internally consistent.
        // Without the signing key the forged + re-chained entries cannot carry
        // valid signatures — verification must fail at the insertion point.
        let mut l = ledger();
        l.append(rec("agent:a1", "t1", "act0")).unwrap();
        l.append(rec("agent:a1", "t1", "act1")).unwrap();
        l.append(rec("agent:a1", "t1", "act2")).unwrap();

        let forged_rec = rec("agent:attacker", "t1", "forged");
        let forged_hash = entry_hash(1, &l.entries[0].hash, &forged_rec);
        let forged = SignedEntry {
            seq: 1,
            prev_hash: l.entries[0].hash.clone(),
            hash: forged_hash,
            // copy a real signature from elsewhere in the chain (replay)
            key_id: l.entries[1].key_id.clone(),
            signature: l.entries[1].signature.clone(),
            record: forged_rec,
        };
        l.entries.insert(1, forged);
        // re-chain the tail: fix seq, prev_hash, hash on every downstream entry
        for i in 2..l.entries.len() {
            l.entries[i].seq = i as u64;
            l.entries[i].prev_hash = l.entries[i - 1].hash.clone();
            l.entries[i].hash = entry_hash(
                l.entries[i].seq,
                &l.entries[i].prev_hash,
                &l.entries[i].record,
            );
        }
        // internally consistent chain, but the replayed signature doesn't cover
        // the forged hash — caught at the insertion point.
        assert!(matches!(l.verify(), ChainStatus::BadSignature(1)));

        // Variant: the attacker re-signs the forged + re-chained tail with their
        // OWN key — caught as untrusted.
        let rogue = SigningKey::generate(&mut OsRng);
        for i in 1..l.entries.len() {
            let h = l.entries[i].hash.clone();
            l.entries[i].key_id = hex::encode(rogue.verifying_key().to_bytes());
            l.entries[i].signature = hex::encode(rogue.sign(h.as_bytes()).to_bytes());
        }
        assert!(matches!(l.verify(), ChainStatus::UntrustedKey(1)));
    }

    #[test]
    fn serde_round_trip_verifies_and_catches_file_tamper() {
        // The on-disk threat model: entries are serialized, the file is (maybe)
        // rewritten, then reloaded and verified WITHOUT the signing ledger —
        // exactly what a platform consumer does.
        let mut l = ledger();
        l.append(rec("agent:a1", "t1", "send_email")).unwrap();
        l.append(rec("agent:a1", "t1", "log_note")).unwrap();
        let (head_seq, head_hash) = {
            let (s, h) = l.head().unwrap();
            (s, h.to_string())
        };
        let trusted = {
            let mut m = HashMap::new();
            let vk = l.signing.verifying_key();
            m.insert(key_id_of(&vk), vk);
            m
        };

        // clean round-trip → Clean, with and without the head checkpoint
        let json = serde_json::to_string(l.entries()).unwrap();
        let reloaded: Vec<SignedEntry> = serde_json::from_str(&json).unwrap();
        assert_eq!(verify_entries(&reloaded, &trusted), ChainStatus::Clean);
        assert_eq!(
            verify_entries_at(&reloaded, &trusted, (head_seq, &head_hash)),
            ChainStatus::Clean
        );

        // tamper with the serialized bytes (the file-rewrite adversary) → caught
        let tampered_json = json.replace("send_email", "wire_funds");
        assert_ne!(tampered_json, json, "tamper must actually change the bytes");
        let tampered: Vec<SignedEntry> = serde_json::from_str(&tampered_json).unwrap();
        assert!(matches!(
            verify_entries(&tampered, &trusted),
            ChainStatus::HashBreak(0)
        ));
    }

    #[test]
    fn hash_test_vector() {
        // Pins the canonical hash-input format (declaration-ordered serde_json of
        // ActionRecord + LE seq + prev_hash + NUL). If this fails, the wire format
        // changed and every existing chain breaks — see the entry_hash docs.
        let r = ActionRecord {
            principal: "agent:acme:reception".into(),
            tenant: "acme".into(),
            kind: "send_email".into(),
            detail: "welcome mail to alice@example.com".into(),
            timestamp_ms: 1_700_000_000_000,
        };
        assert_eq!(
            entry_hash(0, GENESIS, &r),
            // independently derived (python hashlib): sha256(LE(0) || "genesis" || NUL || compact-JSON(record))
            "58d3c40f75ebf6139de85668e34a6b6313488da0ba1924a26f1785aae3b8de8a"
        );
    }
}
