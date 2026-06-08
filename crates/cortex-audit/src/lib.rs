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
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;

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
///
/// **Co-signature (optional, additive non-repudiation).** When the acting principal is an agent
/// with its own key, the entry can carry a *second* signature over the same `hash`:
/// `agent_signature` by `agent_key_id` (the agent's own Ed25519). The box key still signs the
/// chain; the agent co-signs its authorship.
///
/// What it buys (and what it does **not**): a valid co-signature is **unforgeable** — nobody
/// without the agent's private key can produce one, so a compromised *agent/orchestrator* cannot
/// fabricate an action attributed-and-attested to a different agent. It is therefore positive,
/// non-repudiable evidence that the named agent authored the action. It is **not tamper-proof
/// against the box-key holder**: because `hash` covers only `(seq, prev_hash, record)` and not the
/// co-signature fields, whoever holds the box key (the trust root) can *strip* a co-signature
/// (downgrading an entry to box-only) or write a box-only entry attributing any principal — the
/// co-sig defends against compromised agents, not against a compromised box key (if that is
/// compromised the whole ledger is). Both fields default empty (`#[serde(default)]`) so pre-co-sign
/// journals — and box-only entries like human/tenant approvals — read + verify unchanged.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SignedEntry {
    pub seq: u64,
    pub prev_hash: String,
    pub hash: String,
    /// Hex Ed25519 public key id of the signer. Empty ⇒ unsigned ⇒ verification fails.
    pub key_id: String,
    /// Hex Ed25519 signature over `hash`. Empty ⇒ unsigned ⇒ verification fails.
    pub signature: String,
    /// Hex Ed25519 public key id of the **co-signing agent** (its own key). Empty ⇒ no co-sig.
    #[serde(default)]
    pub agent_key_id: String,
    /// Hex Ed25519 **agent** signature over `hash`. Empty ⇒ no co-sig. When either co-sig field
    /// is set, both must be present + valid (and `agent_key_id` trusted) for the chain to verify.
    #[serde(default)]
    pub agent_signature: String,
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
    /// The **agent co-signature** at `seq` did not verify (present but wrong/malformed).
    BadAgentSignature(u64),
    /// The **co-signing agent's** key at `seq` is not in the trusted set (un-issued / unknown).
    UntrustedAgentKey(u64),
    /// The chain verified internally but does not end at the pinned head
    /// checkpoint — entries were truncated/rolled back, or the checkpoint
    /// belongs to a different fork. Carries the expected `(seq, hash)`.
    HeadMismatch {
        expected_seq: u64,
        expected_hash: String,
    },
    /// A durable ledger's journal could not be read while verifying (the chain is
    /// not held resident, so `verify` re-reads from disk). Not a tamper verdict —
    /// the journal was unreadable.
    Io(String),
}

#[derive(Debug, thiserror::Error)]
pub enum AuditError {
    #[error("the ledger has no signing key; an audit entry must be signed")]
    NoSigningKey,
    /// An agent co-signature could not be produced/validated (malformed key/sig, or it did not
    /// verify over the entry hash). The entry is not committed.
    #[error("agent co-signature error: {0}")]
    Signing(String),
    /// A durable ledger's journal could not be read/written (open, append, flush).
    #[error("audit journal I/O: {0}")]
    Io(String),
    /// A recovered journal did not verify clean — it was tampered, truncated
    /// mid-line, or signed by a key this ledger does not trust. Recovery refuses
    /// to continue a chain it cannot vouch for. Carries the failing status.
    #[error("recovered audit journal is not clean: {0:?}")]
    Corrupt(ChainStatus),
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

/// Parse a hex Ed25519 public-key id back into a [`VerifyingKey`] (`None` if malformed / not a
/// valid point).
fn verifying_key_from_hex(hex_id: &str) -> Option<VerifyingKey> {
    let bytes: [u8; 32] = hex::decode(hex_id).ok()?.try_into().ok()?;
    VerifyingKey::from_bytes(&bytes).ok()
}

/// Parse a hex Ed25519 signature (`None` if not 64 bytes of hex).
fn signature_from_hex(hex_sig: &str) -> Option<ed25519_dalek::Signature> {
    let bytes: [u8; 64] = hex::decode(hex_sig).ok()?.try_into().ok()?;
    Some(ed25519_dalek::Signature::from_bytes(&bytes))
}

/// An agent's **co-signature** over an entry's `hash`, returned by the
/// [`append_co_signed`](AuditLedger::append_co_signed) closure: `key_id` is the hex of the agent's
/// Ed25519 public key, `signature` the hex of its signature over the hash. The platform's
/// IdentityService produces this (revealing the agent's vault-sealed key inside the closure).
#[derive(Clone, Debug)]
pub struct AgentCoSignature {
    pub key_id: String,
    pub signature: String,
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
        if let Some(bad) = verify_step(&prev, e, trusted) {
            return bad;
        }
        prev = e.hash.clone();
    }
    ChainStatus::Clean
}

/// The cheap, sequential checks for one entry against the running `prev` hash: chain
/// continuity, recomputed-hash match (tamper), mandatory signature, trusted signer, and
/// signature **decode**. Returns the decoded `(key, signature)` ready for the (expensive)
/// Ed25519 verification, or the failure status. Everything here is fast (SHA-256 + a hex
/// decode); the actual `verify` is split out so it can be batched/parallelized.
fn precheck(
    prev: &str,
    e: &SignedEntry,
    trusted: &HashMap<String, VerifyingKey>,
) -> Result<(VerifyingKey, ed25519_dalek::Signature), ChainStatus> {
    if e.prev_hash != prev {
        return Err(ChainStatus::HashBreak(e.seq));
    }
    // Recompute the hash from the record (incl. principal) — catches tamper.
    if entry_hash(e.seq, &e.prev_hash, &e.record) != e.hash {
        return Err(ChainStatus::HashBreak(e.seq));
    }
    // An audit entry MUST be signed — unsigned is a failure, never clean.
    if e.signature.is_empty() || e.key_id.is_empty() {
        return Err(ChainStatus::Unsigned(e.seq));
    }
    let Some(vk) = trusted.get(&e.key_id) else {
        return Err(ChainStatus::UntrustedKey(e.seq));
    };
    let sig_bytes = match hex::decode(&e.signature)
        .ok()
        .and_then(|b| <[u8; 64]>::try_from(b).ok())
    {
        Some(b) => b,
        None => return Err(ChainStatus::BadSignature(e.seq)),
    };
    Ok((*vk, ed25519_dalek::Signature::from_bytes(&sig_bytes)))
}

/// Verify a single entry against the running `prev` hash + trusted keys (the cheap checks
/// via [`precheck`] plus the Ed25519 verification). Returns `Some(status)` on the first
/// failure, `None` if clean. The one source of per-entry crypto truth for the sequential,
/// in-memory path ([`verify_entries`]); the durable streaming path uses the same [`precheck`]
/// + a parallel batch of the same `verify` ([`verify_chunk`]) so they cannot diverge.
fn verify_step(
    prev: &str,
    e: &SignedEntry,
    trusted: &HashMap<String, VerifyingKey>,
) -> Option<ChainStatus> {
    match precheck(prev, e, trusted) {
        Err(status) => Some(status),
        Ok((vk, sig)) => {
            if vk.verify(e.hash.as_bytes(), &sig).is_err() {
                return Some(ChainStatus::BadSignature(e.seq));
            }
            verify_agent_cosig(e, trusted)
        }
    }
}

/// Verify the **agent co-signature** when present: both co-sig fields must be set, the agent key
/// must be trusted (issued, even if since revoked — past authorship stays verifiable), and the
/// signature must verify over the same `hash` the box key signed. A box-only entry (both fields
/// empty) is unaffected. A half-present co-sig (one field set, the other empty) is malformed.
fn verify_agent_cosig(
    e: &SignedEntry,
    trusted: &HashMap<String, VerifyingKey>,
) -> Option<ChainStatus> {
    if e.agent_key_id.is_empty() && e.agent_signature.is_empty() {
        return None; // box-only entry — no co-signature to check.
    }
    if e.agent_key_id.is_empty() || e.agent_signature.is_empty() {
        return Some(ChainStatus::BadAgentSignature(e.seq)); // half-present ⇒ malformed.
    }
    let Some(avk) = trusted.get(&e.agent_key_id) else {
        return Some(ChainStatus::UntrustedAgentKey(e.seq));
    };
    let Some(asig) = signature_from_hex(&e.agent_signature) else {
        return Some(ChainStatus::BadAgentSignature(e.seq));
    };
    if avk.verify(e.hash.as_bytes(), &asig).is_err() {
        return Some(ChainStatus::BadAgentSignature(e.seq));
    }
    None
}

/// A chunk size for the durable streaming verifier: entries are read + cheap-checked in
/// order, then their signatures verified as a batch. Bounds transient memory (the chunk +
/// its decoded sigs) to `O(chunk)` regardless of journal size, while giving the parallel
/// pass enough work to amortize thread spawn.
const VERIFY_CHUNK: usize = 16_384;
/// Below this many signatures, verify them on the current thread — thread spawn isn't worth
/// it (tiny journals, the common dev/test case).
const PARALLEL_VERIFY_THRESHOLD: usize = 256;

/// The expensive Ed25519 verification for a chunk, **parallelized across cores**: returns the
/// **lowest `seq`** whose signature fails (or `None` if all pass). Each task is independent
/// (the signature is over that entry's own hash), so this is embarrassingly parallel; the
/// chain-continuity (sequential) part is already done by the caller via [`precheck`].
fn min_bad_signature(tasks: &[(VerifyingKey, &str, ed25519_dalek::Signature, u64)]) -> Option<u64> {
    let bad_in = |part: &[(VerifyingKey, &str, ed25519_dalek::Signature, u64)]| {
        part.iter()
            .filter(|(vk, hash, sig, _)| vk.verify(hash.as_bytes(), sig).is_err())
            .map(|t| t.3)
            .min()
    };
    if tasks.len() < PARALLEL_VERIFY_THRESHOLD {
        return bad_in(tasks);
    }
    let nthreads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .min(tasks.len());
    let part_size = tasks.len().div_ceil(nthreads);
    std::thread::scope(|s| {
        let handles: Vec<_> = tasks
            .chunks(part_size)
            .map(|part| s.spawn(move || bad_in(part)))
            .collect();
        handles
            .into_iter()
            .filter_map(|h| h.join().expect("verify thread panicked"))
            .min()
    })
}

/// Verify one in-order chunk against the running `prev`: the cheap checks sequentially
/// (threading `prev`, so chain continuity is exact), then the signatures as a parallel batch.
/// Returns `Err(status)` of the **lowest-seq** failure (identical to a sequential
/// [`verify_step`] pass), or `Ok(())` if the whole chunk is clean.
fn verify_chunk(
    prev_in: &str,
    chunk: &[SignedEntry],
    trusted: &HashMap<String, VerifyingKey>,
) -> Result<(), ChainStatus> {
    let mut prev = prev_in.to_string();
    let mut tasks: Vec<(VerifyingKey, &str, ed25519_dalek::Signature, u64)> =
        Vec::with_capacity(chunk.len());
    let mut seq_fail: Option<ChainStatus> = None;
    // Lowest-seq **agent co-signature** failure in this chunk. Checked inline (co-sigs are the
    // minority — only agent actions carry one); the box signatures stay in the parallel batch.
    // This is the durable counterpart of `verify_step`'s `verify_agent_cosig` call — both paths
    // MUST run it or on-disk recovery would accept a forged co-signature the resident path rejects.
    let mut cosig_fail: Option<ChainStatus> = None;
    for e in chunk {
        match precheck(&prev, e, trusted) {
            Err(status) => {
                // A cheap-check failure breaks the chain here; entries after it are moot.
                // Signature tasks collected so far are all at seq < e.seq.
                seq_fail = Some(status);
                break;
            }
            Ok((vk, sig)) => {
                tasks.push((vk, e.hash.as_str(), sig, e.seq));
                if cosig_fail.is_none() {
                    cosig_fail = verify_agent_cosig(e, trusted); // first ⇒ lowest-seq (in-order)
                }
                prev = e.hash.clone();
            }
        }
    }
    // The chunk's failure is the LOWEST-seq one across the three kinds. `box_fail` and `cosig_fail`
    // are at seq < `seq_fail` (collection stops at the cheap-check break). `min_by_key` returns the
    // FIRST of equal minima, so `box_fail` is placed before `cosig_fail`: on a same-seq tie (one
    // entry with both a bad box sig AND a bad co-sig) the box failure wins — matching `verify_step`,
    // which checks the box signature before the co-signature, so the two paths report identically.
    let box_fail = min_bad_signature(&tasks).map(ChainStatus::BadSignature);
    match [seq_fail, box_fail, cosig_fail]
        .into_iter()
        .flatten()
        .min_by_key(status_seq)
    {
        Some(status) => Err(status),
        None => Ok(()),
    }
}

/// The `seq` a chain-failure status points at (for picking the lowest-seq failure). Non-positional
/// statuses (`Clean`, `HeadMismatch`, `Io`) sort last; they don't arise as per-entry chunk faults.
fn status_seq(s: &ChainStatus) -> u64 {
    match s {
        ChainStatus::HashBreak(n)
        | ChainStatus::BadSignature(n)
        | ChainStatus::UntrustedKey(n)
        | ChainStatus::Unsigned(n)
        | ChainStatus::BadAgentSignature(n)
        | ChainStatus::UntrustedAgentKey(n) => *n,
        _ => u64::MAX,
    }
}

/// Stream a durable journal from disk and verify it entry-by-entry without holding
/// the whole chain in memory (the durable counterpart of [`verify_entries`]).
/// Returns the terminal `(status, last_head)` — `last_head` is the final
/// `(seq, hash)` reached (for the truncation/head check), `None` for an empty chain.
/// `ChainStatus::Io` if the journal can't be read.
fn verify_journal(
    path: &Path,
    trusted: &HashMap<String, VerifyingKey>,
) -> (ChainStatus, Option<(u64, String)>) {
    let file = match File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (ChainStatus::Clean, None),
        Err(e) => {
            return (
                ChainStatus::Io(format!("read {}: {e}", path.display())),
                None,
            )
        }
    };
    let mut prev = GENESIS.to_string();
    let mut head: Option<(u64, String)> = None;
    let mut chunk: Vec<SignedEntry> = Vec::with_capacity(VERIFY_CHUNK);
    let flush = |chunk: &mut Vec<SignedEntry>,
                 prev: &mut String,
                 head: &mut Option<(u64, String)>|
     -> Option<ChainStatus> {
        if chunk.is_empty() {
            return None;
        }
        if let Err(bad) = verify_chunk(prev, chunk, trusted) {
            return Some(bad);
        }
        let last = chunk.last().expect("non-empty");
        *prev = last.hash.clone();
        *head = Some((last.seq, last.hash.clone()));
        chunk.clear();
        None
    };
    for line in BufReader::new(file).lines() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                return (
                    ChainStatus::Io(format!("read {}: {e}", path.display())),
                    head,
                )
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let e: SignedEntry = match serde_json::from_str(&line) {
            Ok(e) => e,
            Err(e) => {
                return (
                    ChainStatus::Io(format!("parse {}: {e}", path.display())),
                    head,
                )
            }
        };
        chunk.push(e);
        if chunk.len() >= VERIFY_CHUNK {
            if let Some(bad) = flush(&mut chunk, &mut prev, &mut head) {
                return (bad, head);
            }
        }
    }
    if let Some(bad) = flush(&mut chunk, &mut prev, &mut head) {
        return (bad, head);
    }
    (ChainStatus::Clean, head)
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
///
/// **Durability.** By default the ledger is in-memory and its trail dies with the
/// process. [`AuditLedger::recover`] makes it **durable**: each append is
/// write-through-appended to an on-disk JSONL journal (one [`SignedEntry`] per
/// line, `write_all` + `flush` — the same on-disk posture as the egress journal),
/// and a fresh process re-loads + re-verifies the journal and **continues the same
/// chain** (so the audit trail survives a restart, binary upgrade, or migration).
/// Truncation of the tail is still only catchable with an out-of-band head pin —
/// see the module docs and [`AuditLedger::verify_at`].
pub struct AuditLedger {
    signing: SigningKey,
    key_id: String,
    trusted: HashMap<String, VerifyingKey>,
    /// **Resident** entries. For an **in-memory** ledger this is the whole chain.
    /// For a **durable** ledger it is the most-recent *window* (the tail, bounded to
    /// [`RESIDENT_CAP`]); older entries live only in the journal and are read on
    /// demand via [`read_range`](AuditLedger::read_range) — so RAM stays flat as the
    /// journal grows, instead of pinning the entire chain. The window always holds the
    /// head, so [`head`](AuditLedger::head)/[`append`] chaining never touch disk.
    entries: Vec<SignedEntry>,
    /// Total entries in the chain (= next `seq`). Distinct from `entries.len()` once
    /// the durable window has evicted older entries.
    total: u64,
    /// Append handle for the durable journal; `None` for an in-memory ledger.
    journal: Option<File>,
    /// Journal path — kept so a durable ledger can re-read for [`read_range`] paging
    /// and streaming [`verify`](AuditLedger::verify). `None` for an in-memory ledger.
    path: Option<std::path::PathBuf>,
    /// Durable resident-window cap (see [`RESIDENT_CAP`]). The window is evicted to this
    /// once it grows to 2×. Configurable via [`AuditLedger::recover_with_cap`].
    resident_cap: usize,
}

/// Default max entries held resident in a **durable** ledger's recent-window. Older
/// entries are evicted from RAM (they stay in the journal, read on demand). Bounds
/// steady-state memory to ~`RESIDENT_CAP` entries regardless of how large the journal
/// grows. In-memory ledgers are unbounded (ephemeral; there is no journal to page from).
pub const RESIDENT_CAP: usize = 4096;

impl AuditLedger {
    /// Open an **in-memory** ledger that signs with `signing`. The signer's own key
    /// is trusted. The trail is not persisted; use [`AuditLedger::recover`] for a
    /// durable, restart-surviving ledger.
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
            total: 0,
            journal: None,
            path: None,
            resident_cap: RESIDENT_CAP,
        }
    }

    /// Open a **durable** ledger backed by the append-only JSONL journal at `path`,
    /// signing with `signing`.
    ///
    /// A missing journal opens an empty chain. An existing journal is loaded and
    /// **re-verified against `signing`'s key** ([`verify_entries`]); recovery
    /// **refuses** ([`AuditError::Corrupt`]) to continue a chain that does not
    /// verify clean — tampered, truncated mid-line, or signed by an untrusted key —
    /// rather than silently appending onto a chain it cannot vouch for. On success
    /// the in-memory chain is the recovered one and the next [`append`] continues
    /// it (correct `seq`/`prev_hash`), write-through to the same file.
    ///
    /// Note: this trusts only `signing`'s own key. A journal whose older entries
    /// were signed by a rotated key must have that key [`trust`](AuditLedger::trust)ed
    /// — recovery across a rotation is intentionally not silent.
    ///
    /// [`append`]: AuditLedger::append
    pub fn recover(path: &Path, signing: SigningKey) -> Result<Self, AuditError> {
        Self::recover_with_cap(path, signing, RESIDENT_CAP)
    }

    /// [`recover`](Self::recover) with an explicit resident-window cap (the number of
    /// most-recent entries kept in RAM; older ones page from disk). Use a small cap in
    /// tests; the default [`RESIDENT_CAP`] otherwise. A larger cap trades RAM for a
    /// bigger no-disk recent view.
    pub fn recover_with_cap(
        path: &Path,
        signing: SigningKey,
        resident_cap: usize,
    ) -> Result<Self, AuditError> {
        Self::recover_with_cap_trusting(path, signing, &[], resident_cap)
    }

    /// [`recover_with_cap`](Self::recover_with_cap) that **also trusts** `agent_keys` while
    /// re-verifying. A journal containing **agent co-signed** entries can only recover clean when
    /// the co-signing agents' public keys are trusted (else `UntrustedAgentKey`) — so the caller
    /// (the platform's IdentityService) supplies the **ever-issued** agent pubkeys here, including
    /// since-revoked ones (revocation stops *new* signing; it does not retroactively invalidate a
    /// past, legitimately-signed entry). The box key is always trusted.
    pub fn recover_with_cap_trusting(
        path: &Path,
        signing: SigningKey,
        agent_keys: &[VerifyingKey],
        resident_cap: usize,
    ) -> Result<Self, AuditError> {
        let cap = resident_cap.max(1);
        let vk = signing.verifying_key();
        let key_id = key_id_of(&vk);
        let mut trusted = HashMap::new();
        trusted.insert(key_id.clone(), vk);
        for ak in agent_keys {
            trusted.insert(key_id_of(ak), *ak);
        }

        // Stream the journal line-by-line — never the whole file as one `String`
        // (a multi-hundred-MB journal otherwise needs that much *contiguous* heap just to
        // load, the acute OOM-on-restart trigger). The whole chain is verified, in
        // `VERIFY_CHUNK`-sized batches whose **signatures verify in parallel** across cores
        // (the dominant cost on restart — see `verify_chunk`), but only the most-recent
        // window is retained. So recovery is O(1) memory in the journal size and roughly
        // Ncores× faster, while still fully re-verifying every entry.
        let mut prev = GENESIS.to_string();
        let mut total: u64 = 0;
        let mut window: Vec<SignedEntry> = Vec::new();
        let mut chunk: Vec<SignedEntry> = Vec::with_capacity(VERIFY_CHUNK);
        let absorb = |chunk: &mut Vec<SignedEntry>,
                      prev: &mut String,
                      total: &mut u64,
                      window: &mut Vec<SignedEntry>|
         -> Result<(), AuditError> {
            if chunk.is_empty() {
                return Ok(());
            }
            verify_chunk(prev, chunk, &trusted).map_err(AuditError::Corrupt)?;
            *prev = chunk.last().expect("non-empty").hash.clone();
            *total += chunk.len() as u64;
            window.append(chunk); // moves entries out; `chunk` is now empty
            if window.len() > 2 * cap {
                let drop = window.len() - cap; // keep the most-recent `cap`
                window.drain(0..drop);
            }
            Ok(())
        };
        match File::open(path) {
            Ok(file) => {
                for (i, line) in BufReader::new(file).lines().enumerate() {
                    let line =
                        line.map_err(|e| AuditError::Io(format!("read {}: {e}", path.display())))?;
                    if line.trim().is_empty() {
                        continue;
                    }
                    let entry: SignedEntry = serde_json::from_str(&line).map_err(|e| {
                        AuditError::Io(format!(
                            "parse journal {} line {}: {e}",
                            path.display(),
                            i + 1
                        ))
                    })?;
                    chunk.push(entry);
                    if chunk.len() >= VERIFY_CHUNK {
                        absorb(&mut chunk, &mut prev, &mut total, &mut window)?;
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(AuditError::Io(format!("read {}: {e}", path.display()))),
        }
        absorb(&mut chunk, &mut prev, &mut total, &mut window)?;

        let journal = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|e| AuditError::Io(format!("open {}: {e}", path.display())))?;

        Ok(Self {
            signing,
            key_id,
            trusted,
            entries: window,
            total,
            journal: Some(journal),
            path: Some(path.to_path_buf()),
            resident_cap: cap,
        })
    }

    /// Trust an additional public key (e.g. a key that signed older entries after rotation).
    pub fn trust(&mut self, vk: VerifyingKey) {
        self.trusted.insert(key_id_of(&vk), vk);
    }

    /// Append an action — signed, with the principal bound into the hash. An audit
    /// entry is **always** signed; there is no unsigned append.
    ///
    /// For a durable ledger the entry is **written to disk first** (write-ahead:
    /// `write_all` + `flush`), and only on a successful write is it added to the
    /// in-memory chain — so a crash never leaves an in-memory entry the journal is
    /// missing. A journal write failure surfaces as [`AuditError::Io`] and the
    /// entry is not recorded.
    pub fn append(&mut self, record: ActionRecord) -> Result<&SignedEntry, AuditError> {
        let (seq, prev_hash, hash) = self.next_hash(&record);
        let signature = self.signing.sign(hash.as_bytes());
        self.commit(SignedEntry {
            seq,
            prev_hash,
            hash,
            key_id: self.key_id.clone(),
            signature: hex::encode(signature.to_bytes()),
            agent_key_id: String::new(),
            agent_signature: String::new(),
            record,
        })
    }

    /// Append an entry **co-signed by the acting agent**. The box key signs the chain (as
    /// [`append`](Self::append)); additionally the caller's `agent_sign` closure signs the SAME
    /// `hash` with the agent's own key and returns its [`AgentCoSignature`]. The agent's public
    /// key is added to the trusted set so this and future [`verify`](Self::verify) calls accept it
    /// (its past entries remain verifiable even after the agent is revoked — revocation is enforced
    /// where the agent *signs*, not in the audit trail). The closure is the only place the agent's
    /// private key is touched, so the ledger never holds it.
    ///
    /// The co-signature is **verified before the entry is committed**: a closure that returns a
    /// bad/malformed signature fails with [`AuditError::Signing`] and nothing is written.
    pub fn append_co_signed<F>(
        &mut self,
        record: ActionRecord,
        agent_sign: F,
    ) -> Result<&SignedEntry, AuditError>
    where
        F: FnOnce(&str) -> Result<AgentCoSignature, AuditError>,
    {
        let (seq, prev_hash, hash) = self.next_hash(&record);
        let signature = self.signing.sign(hash.as_bytes());
        let co = agent_sign(&hash)?;
        // Reconstruct + validate the agent pubkey, and verify the co-signature now (fail-fast: a
        // bad closure must not land an unverifiable entry on the chain).
        let agent_vk = verifying_key_from_hex(&co.key_id)
            .ok_or_else(|| AuditError::Signing("agent co-sign: malformed agent key id".into()))?;
        let agent_sig = signature_from_hex(&co.signature).ok_or_else(|| {
            AuditError::Signing("agent co-sign: malformed agent signature".into())
        })?;
        agent_vk
            .verify(hash.as_bytes(), &agent_sig)
            .map_err(|_| AuditError::Signing("agent co-sign: signature does not verify".into()))?;
        self.trusted.insert(co.key_id.clone(), agent_vk);
        self.commit(SignedEntry {
            seq,
            prev_hash,
            hash,
            key_id: self.key_id.clone(),
            signature: hex::encode(signature.to_bytes()),
            agent_key_id: co.key_id,
            agent_signature: co.signature,
            record,
        })
    }

    /// `(seq, prev_hash, hash)` for the next entry over `record` — the chain position + content
    /// hash both signatures are taken over.
    fn next_hash(&self, record: &ActionRecord) -> (u64, String, String) {
        let seq = self.total;
        let prev_hash = self
            .entries
            .last()
            .map(|e| e.hash.clone())
            .unwrap_or_else(|| GENESIS.to_string());
        let hash = entry_hash(seq, &prev_hash, record);
        (seq, prev_hash, hash)
    }

    /// Commit a freshly-built entry: write-ahead to the journal (so the durable trail is never
    /// behind memory), then push + bound the resident window. A journal write failure surfaces as
    /// [`AuditError::Io`] and the entry is not recorded. Shared by [`append`](Self::append) +
    /// [`append_co_signed`](Self::append_co_signed).
    fn commit(&mut self, entry: SignedEntry) -> Result<&SignedEntry, AuditError> {
        if let Some(file) = self.journal.as_mut() {
            let line = serde_json::to_string(&entry)
                .map_err(|e| AuditError::Io(format!("serialize audit entry: {e}")))?;
            file.write_all(line.as_bytes())
                .and_then(|()| file.write_all(b"\n"))
                .and_then(|()| file.flush())
                .map_err(|e| AuditError::Io(format!("append audit journal: {e}")))?;
        }
        self.entries.push(entry);
        self.total += 1;
        // Durable: bound the resident window — evict the oldest `RESIDENT_CAP` once it grows to 2×
        // (batch eviction → amortized O(1); evicted entries stay in the journal, read on demand via
        // `read_range`). In-memory ledgers keep the whole chain (ephemeral; nowhere to page from).
        if self.journal.is_some() && self.entries.len() > 2 * self.resident_cap {
            self.entries.drain(0..self.resident_cap);
        }
        Ok(self.entries.last().expect("just pushed"))
    }

    /// Verify the whole chain: hash continuity, recomputed-hash match (tamper),
    /// **mandatory** signature, and signature validity against a trusted key. For a
    /// durable ledger (the chain is not held resident) this **re-reads the journal
    /// from disk**, streaming — `O(n)` time, `O(1)` memory; for an in-memory ledger it
    /// verifies the resident chain.
    ///
    /// **Cannot detect truncation** — use [`AuditLedger::verify_at`] with the
    /// out-of-band head checkpoint (see the module docs).
    pub fn verify(&self) -> ChainStatus {
        match &self.path {
            Some(path) => verify_journal(path, &self.trusted).0,
            None => verify_entries(&self.entries, &self.trusted),
        }
    }

    /// [`AuditLedger::verify`], plus the truncation/rollback check against a
    /// pinned `(seq, hash)` head checkpoint (see the module docs). Durable: streams
    /// the journal, then checks the final entry equals the pinned head.
    pub fn verify_at(&self, expected_head: (u64, &str)) -> ChainStatus {
        let Some(path) = &self.path else {
            return verify_entries_at(&self.entries, &self.trusted, expected_head);
        };
        let (status, last) = verify_journal(path, &self.trusted);
        if status != ChainStatus::Clean {
            return status;
        }
        let (expected_seq, expected_hash) = expected_head;
        match last {
            Some((seq, hash)) if seq == expected_seq && hash == expected_hash => ChainStatus::Clean,
            _ => ChainStatus::HeadMismatch {
                expected_seq,
                expected_hash: expected_hash.to_string(),
            },
        }
    }

    /// The `(seq, hash)` head checkpoint after the last append. Consumers record
    /// this out-of-band and later verify with [`AuditLedger::verify_at`] —
    /// without it, truncation of the tail is undetectable. `None` when empty.
    /// Always resident (the window keeps the head), so this never touches disk.
    pub fn head(&self) -> Option<(u64, &str)> {
        self.entries.last().map(|e| (e.seq, e.hash.as_str()))
    }

    /// Total entries in the chain (the full length, not just the resident window).
    pub fn len(&self) -> u64 {
        self.total
    }

    /// Whether the chain is empty.
    pub fn is_empty(&self) -> bool {
        self.total == 0
    }

    /// The **resident** entries: the whole chain for an in-memory ledger, or the most
    /// recent window ([`RESIDENT_CAP`]) for a durable one. For the full history of a
    /// durable ledger, page with [`read_range`](Self::read_range) — `entries()` is not
    /// guaranteed to start at seq 0 once the durable window has evicted older entries.
    pub fn entries(&self) -> &[SignedEntry] {
        &self.entries
    }

    /// Read `[start_seq, start_seq + limit)` from the chain — served from the resident
    /// window when the range falls within it (no disk), else paged from the journal.
    /// The lazy-fetch read path for the audit/board view: recent pages are RAM-cheap,
    /// older pages stream from disk on demand. Owned, since older entries aren't resident.
    pub fn read_range(&self, start_seq: u64, limit: usize) -> Result<Vec<SignedEntry>, AuditError> {
        if limit == 0 || start_seq >= self.total {
            return Ok(Vec::new());
        }
        let end = start_seq.saturating_add(limit as u64).min(self.total);
        let window_start = self.total - self.entries.len() as u64;
        if start_seq >= window_start {
            // Fast path: entirely within the resident window.
            let lo = (start_seq - window_start) as usize;
            let hi = (end - window_start) as usize;
            return Ok(self.entries[lo..hi].to_vec());
        }
        // Older than the window → page from the journal (durable only; an in-memory
        // ledger's window_start is 0 so it never reaches here).
        let Some(path) = &self.path else {
            return Ok(Vec::new());
        };
        let file = File::open(path)
            .map_err(|e| AuditError::Io(format!("read {}: {e}", path.display())))?;
        // The disk path **verifies as it pages**: it threads `prev` from GENESIS and runs
        // `verify_step` on every entry it streams (the scan already starts at seq 0, so this
        // is no extra I/O) — so a journal rewritten *after* recover cannot hand a trusting
        // reader forged history; a tampered entry up to `end` surfaces as `Corrupt`. (Reads
        // served from the resident window are already-verified data, so they need no re-check.)
        let mut prev = GENESIS.to_string();
        let mut out = Vec::new();
        for line in BufReader::new(file).lines() {
            let line = line.map_err(|e| AuditError::Io(format!("read {}: {e}", path.display())))?;
            if line.trim().is_empty() {
                continue;
            }
            let e: SignedEntry = serde_json::from_str(&line)
                .map_err(|e| AuditError::Io(format!("parse {}: {e}", path.display())))?;
            if e.seq >= end {
                break;
            }
            if let Some(bad) = verify_step(&prev, &e, &self.trusted) {
                return Err(AuditError::Corrupt(bad));
            }
            prev = e.hash.clone();
            if e.seq >= start_seq {
                out.push(e);
            }
        }
        Ok(out)
    }

    /// The most recent `limit` entries (newest-last), served from the resident window
    /// when possible. The board's default "recent activity" read.
    pub fn recent(&self, limit: usize) -> Result<Vec<SignedEntry>, AuditError> {
        let start = self.total.saturating_sub(limit as u64);
        self.read_range(start, limit)
    }

    /// Entries for a tenant within the resident window (the board's recent view). For
    /// older history use [`read_range`](Self::read_range) and filter.
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
            agent_key_id: String::new(),
            agent_signature: String::new(),
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

    /// A unique temp path per test invocation (cross-OS; no external tempfile dep).
    fn temp_journal(tag: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "cortex-audit-{}-{tag}-{n}.jsonl",
            std::process::id()
        ))
    }

    #[test]
    fn recover_persists_and_continues_chain_across_restart() {
        // The per-tenant-box requirement: the audit trail survives a restart and the
        // SAME chain continues (no fresh seq-0 chain, no lost tail).
        let path = temp_journal("restart");
        let _ = std::fs::remove_file(&path);
        let key = SigningKey::generate(&mut OsRng);

        // "first boot": durable ledger, two actions.
        let head = {
            let mut l = AuditLedger::recover(&path, key.clone()).unwrap();
            l.append(rec("agent:a1", "t1", "send_email")).unwrap();
            l.append(rec("agent:a1", "t1", "log_note")).unwrap();
            assert_eq!(l.verify(), ChainStatus::Clean);
            let (s, h) = l.head().unwrap();
            (s, h.to_string())
        }; // ledger dropped → process "restart"

        // "second boot": recover from disk, the chain is intact and continues.
        let mut l2 = AuditLedger::recover(&path, key.clone()).unwrap();
        assert_eq!(l2.entries().len(), 2, "the trail was reloaded from disk");
        assert_eq!(l2.head(), Some((head.0, head.1.as_str())), "same head");
        assert_eq!(l2.verify(), ChainStatus::Clean);

        let e = l2.append(rec("agent:a1", "t1", "third")).unwrap();
        assert_eq!(
            e.seq, 2,
            "append continues the recovered chain, not a new one"
        );
        assert_eq!(e.prev_hash, head.1, "chained onto the recovered head");
        assert_eq!(l2.verify(), ChainStatus::Clean);
        assert_eq!(l2.entries().len(), 3);

        // a third boot sees all three, verifying at the pinned head.
        let (s3, h3) = {
            let (s, h) = l2.head().unwrap();
            (s, h.to_string())
        };
        drop(l2);
        let l3 = AuditLedger::recover(&path, key).unwrap();
        assert_eq!(l3.verify_at((s3, &h3)), ChainStatus::Clean);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn recover_refuses_a_tampered_journal() {
        // A box whose on-disk trail was rewritten must NOT silently continue it.
        let path = temp_journal("tamper");
        let _ = std::fs::remove_file(&path);
        let key = SigningKey::generate(&mut OsRng);
        {
            let mut l = AuditLedger::recover(&path, key.clone()).unwrap();
            l.append(rec("agent:a1", "t1", "send_email")).unwrap();
            l.append(rec("agent:a1", "t1", "log_note")).unwrap();
        }
        // rewrite the file: flip a signed field in the persisted JSON.
        let text = std::fs::read_to_string(&path).unwrap();
        let tampered = text.replace("send_email", "wire_funds");
        assert_ne!(tampered, text, "tamper must change the bytes");
        std::fs::write(&path, tampered).unwrap();

        match AuditLedger::recover(&path, key) {
            Err(AuditError::Corrupt(ChainStatus::HashBreak(0))) => {}
            Err(e) => panic!("expected Corrupt(HashBreak(0)), got {e:?}"),
            Ok(_) => panic!("recovery must refuse a tampered journal"),
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn recover_refuses_a_journal_signed_by_an_untrusted_key() {
        // The journal was written by key A; a box that only trusts key B must not
        // adopt it (a foreign/forged trail).
        let path = temp_journal("foreign");
        let _ = std::fs::remove_file(&path);
        let key_a = SigningKey::generate(&mut OsRng);
        {
            let mut l = AuditLedger::recover(&path, key_a).unwrap();
            l.append(rec("agent:a1", "t1", "act")).unwrap();
        }
        let key_b = SigningKey::generate(&mut OsRng);
        match AuditLedger::recover(&path, key_b) {
            Err(AuditError::Corrupt(ChainStatus::UntrustedKey(0))) => {}
            Err(e) => panic!("expected Corrupt(UntrustedKey(0)), got {e:?}"),
            Ok(_) => panic!("recovery must refuse an untrusted-key journal"),
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn durable_append_is_on_disk_immediately() {
        // Write-ahead: after append returns, the entry is already in the journal
        // (no buffering window where a crash loses it).
        let path = temp_journal("ondisk");
        let _ = std::fs::remove_file(&path);
        let key = SigningKey::generate(&mut OsRng);
        let mut l = AuditLedger::recover(&path, key.clone()).unwrap();
        l.append(rec("agent:a1", "t1", "send_email")).unwrap();

        // read it back with an independent verifier (the "second box"): one entry,
        // verifies clean against the key.
        let text = std::fs::read_to_string(&path).unwrap();
        let reloaded: Vec<SignedEntry> = text
            .lines()
            .filter(|s| !s.trim().is_empty())
            .map(|s| serde_json::from_str(s).unwrap())
            .collect();
        assert_eq!(reloaded.len(), 1);
        let trusted = {
            let mut m = HashMap::new();
            let vk = key.verifying_key();
            m.insert(key_id_of(&vk), vk);
            m
        };
        assert_eq!(verify_entries(&reloaded, &trusted), ChainStatus::Clean);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn recover_missing_journal_starts_empty() {
        let path = temp_journal("missing");
        let _ = std::fs::remove_file(&path);
        let key = SigningKey::generate(&mut OsRng);
        let l = AuditLedger::recover(&path, key).unwrap();
        assert_eq!(l.entries().len(), 0);
        assert_eq!(l.head(), None);
        assert_eq!(l.verify(), ChainStatus::Clean);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn durable_window_bounds_resident_and_pages_full_history() {
        // The L1b property: a durable ledger past the window cap keeps only a bounded
        // recent window resident (RAM flat in chain length), while the FULL history is
        // still reachable + verifiable from disk.
        let path = temp_journal("window");
        let _ = std::fs::remove_file(&path);
        let key = SigningKey::generate(&mut OsRng);
        let cap = 8; // tiny cap so a handful of appends crosses eviction (fast in debug)
        let n = 2 * cap + 5; // forces at least one eviction
        let mut l = AuditLedger::recover_with_cap(&path, key, cap).unwrap();
        for i in 0..n {
            l.append(rec("agent:a1", "t1", &format!("act{i}"))).unwrap();
        }
        assert_eq!(l.len(), n as u64, "len() is the full chain");
        assert!(
            l.entries().len() <= 2 * cap,
            "resident window is bounded ({} entries)",
            l.entries().len()
        );
        assert!(l.entries().len() < n, "older entries were evicted from RAM");
        assert_eq!(l.head().unwrap().0, (n - 1) as u64, "head is the true last");
        assert_eq!(l.verify(), ChainStatus::Clean, "streamed full verify");

        // Page the OLDEST entries (below the window) — must come from disk, correct.
        let oldest = l.read_range(0, 3).unwrap();
        assert_eq!(oldest.len(), 3);
        assert_eq!(oldest[0].seq, 0);
        assert_eq!(oldest[0].record.kind, "act0");
        assert_eq!(oldest[2].seq, 2);

        // recent() serves the newest from the resident window.
        let recent = l.recent(2).unwrap();
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[1].seq, (n - 1) as u64);
        assert_eq!(recent[1].record.kind, format!("act{}", n - 1));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn recover_bounds_window_and_continues_after_eviction() {
        // Restart of a large durable ledger: recovery rebuilds a BOUNDED window (not
        // the whole chain), keeps the right total, verifies, pages full history, and a
        // subsequent append continues the chain with the correct seq.
        let path = temp_journal("recover-window");
        let _ = std::fs::remove_file(&path);
        let key = SigningKey::generate(&mut OsRng);
        let cap = 8;
        let n = 2 * cap + 5;
        {
            let mut l = AuditLedger::recover_with_cap(&path, key.clone(), cap).unwrap();
            for i in 0..n {
                l.append(rec("agent:a1", "t1", &format!("a{i}"))).unwrap();
            }
        }
        let mut l2 = AuditLedger::recover_with_cap(&path, key, cap).unwrap();
        assert_eq!(l2.len(), n as u64);
        assert!(l2.entries().len() <= 2 * cap, "recover bounds the window");
        assert_eq!(l2.verify(), ChainStatus::Clean);
        assert_eq!(
            l2.read_range(0, 1).unwrap()[0].seq,
            0,
            "seq 0 still on disk"
        );
        let e = l2.append(rec("agent:a1", "t1", "next")).unwrap();
        assert_eq!(e.seq, n as u64, "append continues after recover+eviction");
        assert_eq!(l2.verify(), ChainStatus::Clean);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn verify_chunk_threads_prev_and_reports_lowest_seq_failure() {
        // Directly exercise the chunk verifier's continuity + lowest-seq logic (the parts
        // the streaming loop relies on across VERIFY_CHUNK boundaries) without a >16k fixture.
        let key = SigningKey::generate(&mut OsRng);
        let trusted = {
            let mut m = HashMap::new();
            let vk = key.verifying_key();
            m.insert(key_id_of(&vk), vk);
            m
        };
        let mut l = AuditLedger::new(key);
        for i in 0..6 {
            l.append(rec("agent:a1", "t1", &format!("a{i}"))).unwrap();
        }
        let entries = l.entries().to_vec();

        // whole chunk clean
        assert_eq!(verify_chunk(GENESIS, &entries, &trusted), Ok(()));

        // cross-boundary prev threading: a clean second half continues from the first half's
        // head, and a WRONG boundary prev is caught at the first entry of the second half.
        assert_eq!(verify_chunk(GENESIS, &entries[0..3], &trusted), Ok(()));
        let mid = entries[2].hash.clone();
        assert_eq!(verify_chunk(&mid, &entries[3..6], &trusted), Ok(()));
        assert_eq!(
            verify_chunk("not-the-real-prev", &entries[3..6], &trusted),
            Err(ChainStatus::HashBreak(3))
        );

        // mixed failures in one chunk: a bad signature at seq 2 and a hash tamper at seq 4 →
        // the LOWER seq (the bad sig) is reported, matching a sequential pass.
        let mut tampered = entries;
        let mut sig: Vec<char> = tampered[2].signature.chars().collect();
        sig[0] = if sig[0] == '0' { '1' } else { '0' };
        tampered[2].signature = sig.into_iter().collect();
        tampered[4].record.detail = "tampered".into();
        assert_eq!(
            verify_chunk(GENESIS, &tampered, &trusted),
            Err(ChainStatus::BadSignature(2))
        );
    }

    #[test]
    fn parallel_recover_verifies_clean_and_reports_lowest_bad_signature() {
        // Exercise the parallel signature-verify path (> PARALLEL_VERIFY_THRESHOLD entries):
        // a clean chain recovers, and a chain with TWO bad signatures is refused at the
        // LOWEST seq — identical to a sequential verify, despite cross-thread verification.
        let path = temp_journal("parallel");
        let _ = std::fs::remove_file(&path);
        let key = SigningKey::generate(&mut OsRng);
        let n = PARALLEL_VERIFY_THRESHOLD + 8; // forces the multi-threaded batch
        {
            let mut l = AuditLedger::recover_with_cap(&path, key.clone(), 16).unwrap();
            for i in 0..n {
                l.append(rec("agent:a1", "t1", &format!("act{i}"))).unwrap();
            }
        }
        // clean chain recovers + verifies through the parallel path
        let l = AuditLedger::recover_with_cap(&path, key.clone(), 16).unwrap();
        assert_eq!(l.len(), n as u64);
        assert_eq!(l.verify(), ChainStatus::Clean);
        drop(l);

        // Corrupt the signatures of two entries (seq 5 and seq 200, both valid 128-hex so
        // they pass decode and fail at the Ed25519 verify — the parallel branch).
        let mut entries: Vec<SignedEntry> = std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        let flip = |s: &str| -> String {
            let mut c: Vec<char> = s.chars().collect();
            c[0] = if c[0] == '0' { '1' } else { '0' };
            c.into_iter().collect()
        };
        entries[5].signature = flip(&entries[5].signature);
        entries[200].signature = flip(&entries[200].signature);
        let rewritten: String = entries
            .iter()
            .map(|e| serde_json::to_string(e).unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(&path, rewritten).unwrap();

        // recover must refuse, reporting the LOWEST bad seq (5), not 200.
        match AuditLedger::recover_with_cap(&path, key, 16) {
            Err(AuditError::Corrupt(ChainStatus::BadSignature(5))) => {}
            Err(e) => panic!("expected Corrupt(BadSignature(5)), got {e:?}"),
            Ok(_) => panic!("recovery must refuse a chain with bad signatures"),
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn read_range_disk_path_catches_post_recover_tamper() {
        // The L1b read path is a verification boundary: after a clean recover, an
        // attacker who rewrites the on-disk journal must not be able to hand a paging
        // reader forged history — read_range verifies as it pages from disk.
        let path = temp_journal("read-tamper");
        let _ = std::fs::remove_file(&path);
        let key = SigningKey::generate(&mut OsRng);
        let cap = 8;
        let n = 2 * cap + 5; // seq 0 is well below the resident window
        let l = {
            let mut l = AuditLedger::recover_with_cap(&path, key.clone(), cap).unwrap();
            for i in 0..n {
                l.append(rec("agent:a1", "t1", &format!("act{i}"))).unwrap();
            }
            l
        };
        // a clean below-window page is fine
        assert_eq!(l.read_range(0, 1).unwrap()[0].record.kind, "act0");

        // rewrite the journal on disk (the file-rewrite adversary), flipping seq 0.
        let text = std::fs::read_to_string(&path).unwrap();
        let tampered = text.replacen("act0", "wire_funds", 1);
        assert_ne!(tampered, text, "tamper must change the bytes");
        std::fs::write(&path, tampered).unwrap();

        // paging that entry from disk now fails closed — not silently forged.
        match l.read_range(0, 1) {
            Err(AuditError::Corrupt(ChainStatus::HashBreak(0))) => {}
            other => panic!("read_range must reject tampered disk data, got {other:?}"),
        }
        let _ = std::fs::remove_file(&path);
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

    // --- agent co-signature (W2.A1) ---

    /// A closure that co-signs an entry's `hash` with `agent` (the IdentityService's role in prod).
    fn cosign_with(
        agent: &SigningKey,
    ) -> impl FnOnce(&str) -> Result<AgentCoSignature, AuditError> + '_ {
        move |hash: &str| {
            Ok(AgentCoSignature {
                key_id: key_id_of(&agent.verifying_key()),
                signature: hex::encode(agent.sign(hash.as_bytes()).to_bytes()),
            })
        }
    }

    #[test]
    fn co_signed_entry_carries_both_signatures_and_verifies() {
        let mut l = ledger();
        let agent = SigningKey::generate(&mut OsRng);
        l.append_co_signed(
            rec("agent:acme:reception", "acme", "send"),
            cosign_with(&agent),
        )
        .unwrap();
        let e = &l.entries[0];
        assert!(!e.signature.is_empty(), "box-key chain signature present");
        assert_eq!(e.agent_key_id, key_id_of(&agent.verifying_key()));
        assert!(!e.agent_signature.is_empty(), "agent co-signature present");
        // Both signatures verify; the chain is clean (append_co_signed auto-trusts the agent key).
        assert_eq!(l.verify(), ChainStatus::Clean);
    }

    #[test]
    fn co_signed_entry_fails_when_the_agent_key_is_not_trusted() {
        let mut l = ledger();
        let agent = SigningKey::generate(&mut OsRng);
        l.append_co_signed(
            rec("agent:acme:reception", "acme", "send"),
            cosign_with(&agent),
        )
        .unwrap();
        // Verify against a trust set that holds ONLY the box key (the agent key was never issued
        // to this verifier) → the co-signature can't be vouched for.
        let mut box_only = HashMap::new();
        box_only.insert(l.key_id.clone(), l.signing.verifying_key());
        assert_eq!(
            verify_entries(&l.entries, &box_only),
            ChainStatus::UntrustedAgentKey(0)
        );
    }

    #[test]
    fn tampered_agent_signature_is_caught() {
        let mut l = ledger();
        let agent = SigningKey::generate(&mut OsRng);
        l.append_co_signed(
            rec("agent:acme:reception", "acme", "send"),
            cosign_with(&agent),
        )
        .unwrap();
        // Replace the agent signature with a valid-shaped but wrong one (sign different bytes).
        l.entries[0].agent_signature = hex::encode(agent.sign(b"not the hash").to_bytes());
        assert_eq!(l.verify(), ChainStatus::BadAgentSignature(0));
    }

    #[test]
    fn half_present_cosignature_is_malformed() {
        let mut l = ledger();
        let agent = SigningKey::generate(&mut OsRng);
        l.append_co_signed(
            rec("agent:acme:reception", "acme", "send"),
            cosign_with(&agent),
        )
        .unwrap();
        // key id set, signature cleared → malformed co-sig, not "box-only".
        l.entries[0].agent_signature.clear();
        assert_eq!(l.verify(), ChainStatus::BadAgentSignature(0));
    }

    #[test]
    fn append_co_signed_rejects_a_bad_closure_and_commits_nothing() {
        let mut l = ledger();
        let agent = SigningKey::generate(&mut OsRng);
        // Closure returns a signature over the WRONG bytes — it won't verify over the hash.
        let bad = |_hash: &str| {
            Ok(AgentCoSignature {
                key_id: key_id_of(&agent.verifying_key()),
                signature: hex::encode(agent.sign(b"wrong").to_bytes()),
            })
        };
        let err = l.append_co_signed(rec("agent:acme:reception", "acme", "send"), bad);
        assert!(matches!(err, Err(AuditError::Signing(_))), "{err:?}");
        assert_eq!(l.entries.len(), 0, "a rejected co-sign commits nothing");
        assert_eq!(l.total, 0);
    }

    #[test]
    fn durable_verify_catches_a_tampered_agent_cosignature() {
        // The DURABLE path (verify_journal → verify_chunk) must reject a forged co-signature, not
        // just the in-memory verify_step. (Regression for the crypto-review finding: the two
        // verification paths must not diverge.)
        let path = temp_journal("cosig-tamper");
        let _ = std::fs::remove_file(&path);
        let key = SigningKey::generate(&mut OsRng);
        let agent = SigningKey::generate(&mut OsRng);

        let mut l = AuditLedger::recover_with_cap(&path, key, 8).unwrap();
        l.append_co_signed(
            rec("agent:acme:reception", "acme", "send"),
            cosign_with(&agent),
        )
        .unwrap();
        let good_sig = l.entries[0].agent_signature.clone();
        assert_eq!(
            l.verify(),
            ChainStatus::Clean,
            "durable verify, agent trusted"
        );

        // Rewrite the on-disk agent_signature to a valid-shaped but wrong signature, then
        // re-verify DURABLY (verify() re-reads the journal from disk).
        let text = std::fs::read_to_string(&path).unwrap();
        let bad_sig = hex::encode(agent.sign(b"not the hash").to_bytes());
        let tampered = text.replace(&good_sig, &bad_sig);
        assert_ne!(tampered, text, "tamper must change the bytes");
        std::fs::write(&path, tampered).unwrap();

        assert_eq!(
            l.verify(),
            ChainStatus::BadAgentSignature(0),
            "the durable path must catch the forged co-signature"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn recover_needs_the_agent_key_trusted_for_a_co_signed_journal() {
        // A co-signed journal recovered with only the box key trusted is refused (UntrustedAgentKey)
        // — recovery won't vouch for an agent it doesn't know. Supplying the issued agent key via
        // `recover_with_cap_trusting` recovers it clean. (Past entries of a since-revoked agent
        // still verify, because the caller passes the ever-issued set.)
        let path = temp_journal("cosig-recover");
        let _ = std::fs::remove_file(&path);
        let key = SigningKey::generate(&mut OsRng);
        let agent = SigningKey::generate(&mut OsRng);
        {
            let mut l = AuditLedger::recover_with_cap(&path, key.clone(), 8).unwrap();
            l.append_co_signed(
                rec("agent:acme:reception", "acme", "send"),
                cosign_with(&agent),
            )
            .unwrap();
        }
        // Box-only trust → the co-signing agent is unknown → recovery refuses.
        match AuditLedger::recover_with_cap(&path, key.clone(), 8) {
            Err(AuditError::Corrupt(ChainStatus::UntrustedAgentKey(0))) => {}
            Err(e) => panic!("expected Corrupt(UntrustedAgentKey(0)), got {e:?}"),
            Ok(_) => {
                panic!("recovery must refuse a co-signed journal without the agent key trusted")
            }
        }
        // Re-supply the issued agent key → recovers clean and continues.
        let l = AuditLedger::recover_with_cap_trusting(&path, key, &[agent.verifying_key()], 8)
            .unwrap();
        assert_eq!(l.verify(), ChainStatus::Clean);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn both_verify_paths_agree_on_a_double_fault_entry() {
        // One entry with BOTH a bad box signature and a bad agent co-signature. The in-memory
        // (`verify_step`) and durable-chunk (`verify_chunk`) paths must report the SAME status —
        // the box failure (checked first) wins the same-seq tie on both. (Regression for the
        // tie-break-order divergence: min_by_key returns the first of equal minima.)
        let mut l = ledger();
        let agent = SigningKey::generate(&mut OsRng);
        l.append_co_signed(
            rec("agent:acme:reception", "acme", "send"),
            cosign_with(&agent),
        )
        .unwrap();
        // Corrupt both signatures (valid-shaped, wrong bytes).
        l.entries[0].signature = hex::encode(l.signing.sign(b"wrong-box").to_bytes());
        l.entries[0].agent_signature = hex::encode(agent.sign(b"wrong-agent").to_bytes());

        let in_memory = verify_entries(&l.entries, &l.trusted);
        let durable_chunk = match verify_chunk(GENESIS, &l.entries, &l.trusted) {
            Ok(()) => ChainStatus::Clean,
            Err(s) => s,
        };
        assert_eq!(in_memory, ChainStatus::BadSignature(0));
        assert_eq!(
            in_memory, durable_chunk,
            "verify_step and verify_chunk must not diverge on the same entry"
        );
    }

    #[test]
    fn box_only_and_co_signed_entries_coexist_on_one_chain() {
        // Backward-compat + mixed chain: a human approval (box-only) and an agent action
        // (co-signed) verify together.
        let mut l = ledger();
        let agent = SigningKey::generate(&mut OsRng);
        l.append(rec("human:owner", "acme", "egress.approved"))
            .unwrap();
        l.append_co_signed(
            rec("agent:acme:reception", "acme", "send"),
            cosign_with(&agent),
        )
        .unwrap();
        assert!(l.entries[0].agent_signature.is_empty(), "box-only entry");
        assert!(!l.entries[1].agent_signature.is_empty(), "co-signed entry");
        assert_eq!(l.verify(), ChainStatus::Clean);
    }
}
