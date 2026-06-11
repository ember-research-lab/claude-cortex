# Crypto consolidation — status & follow-up

Goal: the org has **one** sign/verify/hash surface — [`ember-crypto`](https://github.com/ember-research-lab/ember-crypto) — instead of each crate calling `ed25519-dalek` + `sha2` directly. cortex stays on Ed25519 for substrate compatibility (pulls ember-crypto with `default-features = false`, so no post-quantum code compiles in).

## Done

- **cortex-core** (`signing.rs`) — ledger keygen / sign / verify and `compute_key_id` now delegate to `ember-crypto`. Byte-identical: `.private_key` stays the raw 32-byte seed, key-id construction unchanged, Ed25519 signatures are deterministic so the bytes match, verification accepts the same set. v2-fixture substrate tests (block hashes + merkle root) are unaffected. `ed25519-dalek` and `rand` dropped from `cortex-core`. (PR that introduced this.)

## Follow-up — cortex-audit (NOT done, deliberate)

`cortex-audit` still depends on `ed25519-dalek` directly. It was left out of the cortex-core PR on purpose because it isn't a mechanical swap:

- It verifies **batches** with a bespoke **min-bad-signature bisection** (`min_bad_signature` over `Vec<(VerifyingKey, &str, Signature, u64)>` in `crates/cortex-audit/src/lib.rs`) to locate the first bad block efficiently.
- It uses **hex-encoded** signatures (`signature_from_hex`), not base64.

ember-crypto's `Ed25519Scheme.verify` is one-signature-at-a-time, so consolidating audit needs a design decision:

1. Add a batch-capable entry point to ember-crypto and port the bisection on top of it.
2. **(lowest risk)** Keep the bisection in audit; replace only the *leaf* `verify` with `ember_crypto::Ed25519Scheme`, drop `ed25519-dalek` from `cortex-audit`. Preserve hex sig encoding + identical batch behavior.
3. Leave audit as-is and accept a direct dalek dep there.

**Acceptance** (if pursued): cortex-audit no longer depends on `ed25519-dalek` directly (or a documented reason it must); batch-verify + min-bad-signature results unchanged; fmt / clippy `-D warnings` / `cargo test --workspace` / `cargo deny` green on 3-OS CI.

Not urgent — audit works today. This is crypto-surface consolidation; schedule when convenient.
