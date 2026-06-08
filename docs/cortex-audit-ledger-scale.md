# cortex-audit — ledger scale: segmentation + bounded recovery (design)

**Status:** proposal · 2026-06-07 · driven by the ember-smb-platform
per-business-per-server deployment (one tenant's action-audit ledger growing under
daily 9-5 use, replicated across ~100 boxes). Co-consumer: Whale Signal.

**TL;DR.** The action ledger holds the **entire chain in RAM** (`entries: Vec<SignedEntry>`,
never shrinks) and **re-reads + re-verifies the whole journal on every restart**
(`recover` = `read_to_string` of the whole file → parse all → verify every signature).
Disk is cheap and not the problem; **RAM and restart-time are the cliff**. Fix in three
additive layers that preserve the on-disk entry format and the hash-chain/signature
semantics (substrate stays inviolable): (1) bounded-RAM recovery, (2) segmentation +
sealed checkpoints, (3) archival.

---

## 1. The measured problem

Benchmark: `crates/cortex-audit/tests/ledger_scale.rs` (release, WSL2). Build N realistic
action entries through the real write-through append path, drop, then `recover` from disk
(the exact cost a daemon pays on every restart).

| N entries | disk | bytes/entry | recover time | recover rate | peak RSS | RAM/entry |
|---|---|---|---|---|---|---|
| 100,000 | 53.8 MiB | 564 B | 3.1 s | 31.8k/s | 128 MiB | ~1.3 KB |
| 1,000,000 | 538.7 MiB | 565 B | 31.2 s | 32.0k/s | 1,258 MiB | ~1.3 KB |

Both costs are **linear and stable**: ~565 B/entry on disk, **~1.3 KB/entry resident**,
**~32k entries/s recovery** (parse + alloc bound, *not* just signature verify — so cheaper
signatures alone would not fix recovery).

**Resident-RAM horizon** (the steady state a long-running daemon pins — every append stays
in the Vec):

| Resident | entries |
|---|---|
| 2 GiB | ~1.63M |
| 4 GiB | ~3.26M |
| 8 GiB | ~6.51M |

**Operational translation.** An active SMB ledgers ~600–1,000 actions/day (every
orchestrator decision, delegation, tool/connector call, egress submit+resolve, workflow
frame+effect, SoR write) → **~200k entries/year** typical, **~500k–750k/year heavy**
(lots of lead-gen + busy agents).

- *Typical* business: fine for years (year 5 ≈ 1M → 31 s recover, 1.25 GiB).
- *Heavy* business on a small VM (CPX31, ~8 GiB shared with OS + daemon + SoR + browser
  sidecar + model context): **~2–4 years** to cross 2–4 GiB resident and ~1 min recovery —
  while it is a paying customer.

**Two failure modes:**
1. **RAM starvation** — the ledger Vec crowds everything else on the box.
2. **OOM crash-loop** (the dangerous one) — a memory-pressured box restarts, `recover` does
   a multi-hundred-MB `read_to_string` + rebuilds the >1 GiB Vec, OOMs again → loop. Same
   failure class as the `whale_signal.db` incidents.

> **Consumer amplification (SMB).** The SMB `adapter-ledger` keeps a *second* full `Vec` of
> the chain (it converts every `cortex_audit::SignedEntry` into the platform's own type at
> recover and on each append). So the real daemon holds the chain **twice** — the resident
> numbers above roughly double in the live SMB process. Bounding the resident chain in
> cortex-audit (L1b) plus dropping the adapter's parallel copy are both needed.

---

## 2. Constraint — substrate stays inviolable

The on-disk **`SignedEntry` JSONL line format**, the **SHA-256 hash chain**, and the
**Ed25519 signature semantics** do not change. Everything below is **additive storage
layout + recovery mechanism + in-memory strategy**; a new ledger reads an old single-file
journal without migration.

**Non-goals:** changing the entry format, the hash/sig scheme, or verification semantics
(tamper/truncation guarantees are preserved, including the existing out-of-band head pin
`verify_at`).

---

## 3. Design (three layers, priority order)

Layer 1 splits into **L1a (streaming recovery)** — landed, non-breaking — and **L1b (bounded
resident chain)** — the larger RAM win, gated on a product decision.

#### L1a — streaming recovery  ✅ **DONE** (this change)

Replace `read_to_string` (whole journal → one contiguous `String`) with a buffered
line-by-line reader: parse + collect one line at a time, then verify the chain exactly as
before (`verify_entries`, semantics unchanged). Removes the single large contiguous
allocation — the acute OOM-on-restart trigger — with **no API change** and **identical
verification semantics** (all 15 cortex-audit tests unchanged).

Measured A/B, recover-only on the same 1M-entry journal (release):

| recover @ 1M | peak RSS | rate |
|---|---|---|
| before (`read_to_string`) | 1258 MiB | 27.8k/s |
| after (streaming) | **720 MiB** | 31.7k/s |

The 538 MiB removed is exactly the whole-file `String` (= journal size). The remaining
720 MiB is the resident chain `Vec` — L1b's target.

#### L1b — bounded resident chain  ✅ **DONE** (this change)

The whole chain is no longer materialized in RAM. A durable ledger keeps only the signing/
trusted keys, a `total` counter (the seq source + `len()`), the journal path, and a
**bounded recent window** (`RESIDENT_CAP`, default 4096; configurable via
`recover_with_cap`). The window always holds the head, so `head()`/`append` chaining never
touch disk.

- **Recovery** streams + verifies every entry against the running `prev_hash` and keeps only
  the last window (batch eviction at 2× → amortized O(1)). RAM → `O(window)`, not `O(n)`.
- **Append** advances `total`, pushes, evicts the oldest past 2× the cap; write-through
  unchanged.
- **Reads** are now `read_range(start_seq, limit)` / `recent(limit)` (owned, lazy: served from
  the window when in range, else paged from the journal) + `len()`. `entries()` still exists
  but returns the **resident window** (documented). The only direct consumer is the SMB
  `adapter-ledger` — Whale Signal hasn't adopted cortex-audit yet, so this *sets* the contract.
  The disk read path **verifies as it pages** (threads `prev` from genesis, runs the per-entry
  check on each streamed entry — no extra I/O, the scan already starts at seq 0), so a journal
  rewritten *after* recover can't hand a paging reader forged history; window reads are
  already-verified resident data. (Crypto-review of this change: tamper-evidence preserved.)
- **In-memory ledgers are unchanged** (full chain resident; nowhere to page from) — the entire
  crypto/tamper test battery is untouched and green.

Measured @ 1M entries (release): **peak RSS 720 → 9.3 MiB** (vs 1258 MiB pre-L1a) — a ~135×
cut, and **flat as the journal grows** (resident = the window, not the chain). Recovery
*time* is still `O(n)` (~29 s @ 1M — it re-verifies the whole chain); bounding that is L2.

### Layer 2 — segmentation + sealed checkpoints  *(kills restart-time)*

Roll the journal into **sealed segments** (`audit-000001.jsonl`, …) at a size/time boundary
(e.g. 100k entries or daily). On seal, append a **signed checkpoint** `(segment, end_seq,
head_hash)` to a `manifest.jsonl`. The checkpoint primitive already exists
(`verify_at` / `verify_entries_at` with an out-of-band head).

- **Recovery verifies only the open segment** and **trusts prior segments' sealed
  checkpoints** (each was fully verified when sealed). Recover time → `O(open segment)`,
  bounded forever (e.g. ≤100k → ~3 s, flat, regardless of total history).
- **`verify --full`** re-verifies all segments end-to-end for an actual tamper investigation
  or regulator audit (the expensive path, run on demand, not on every boot).
- The manifest is itself a tiny hash-chained, signed list — tampering with or dropping a
  sealed segment is caught by the manifest checkpoint chain.

### Layer 3 — archival + retention

Sealed, checkpoint-anchored segments are immutable → **compress + ship to per-tenant
encrypted cold storage**; the hot box keeps the last K segments + the full manifest. Long-term
/ regulatory retention is satisfied by the archive + the checkpoint chain, without growing the
box. **Crypto-shredding** a segment's archive key gives right-to-erasure against an otherwise
append-only ledger (resolves SMB plan open-Q #9 residency/erasure; pairs with an external
anchor for regulator-grade tamper-evidence, open-Q #6).

---

## 4. Compatibility & migration

- A **legacy single-file journal** is treated as one (open) segment; the first seal rolls it
  forward. No migration step, no format change — existing journals load as-is.
- cortex-audit is the **only** reader of its journal, so the manifest/segment layout is an
  internal evolution; consumers see it only through the (iterator-shaped) public API of
  Layer 1.

## 5. Phasing

| Phase | Deliverable | Status |
|---|---|---|
| **L1a** | Streaming recovery (no whole-file `String`) | ✅ done — non-breaking, −43% recover RSS |
| **L1b** | Bounded resident window + paged read API (`read_range`/`recent`/`len`) | ✅ done — peak RSS 1258→9.3 MiB @ 1M, flat in N |
| **L1b (SMB)** | Bump cortex-audit rev; adapter drops its parallel `Vec` + pages; board view uses `recent`/`read_range` | next — SMB-side, no sign-off blocker (adapter is the only consumer) |
| **L2** | Segmentation + sealed checkpoints + `verify --full` | bounds **restart-time** to a constant (recovery is still O(n) after L1b) |
| **L3** | Archival to cold storage + crypto-shred erasure | long-horizon retention + residency; ops-heavy, least urgent |

## 6. Open questions (cross-consumer — needs Whale Signal sign-off)

1. Segment boundary: entry-count vs time vs size? (Lean: count, e.g. 100k — predictable
   recover cost.)
2. Tail-window size default + whether it's per-consumer configurable.
3. The `entries()` → iterator API change: do we break it, or add a paged method and deprecate
   the slice? (SMB adapter + board view are the affected call sites.)
4. Manifest anchoring: is the local signed manifest enough, or do we want an external anchor
   (third-party root hash) for regulator-grade tamper-evidence from the start?
5. Do L1's RAM bounds belong in cortex-audit (all consumers benefit) or as a consumer opt-in?
   (Recommend: in cortex-audit — Whale Signal will hit the same wall.)

## 7. Reproduce the numbers

```sh
LEDGER_SCALE_N=100000  cargo test --release -p cortex-audit --test ledger_scale -- --ignored --nocapture
LEDGER_SCALE_N=1000000 cargo test --release -p cortex-audit --test ledger_scale -- --ignored --nocapture
LEDGER_SCALE_N=5000000 cargo test --release -p cortex-audit --test ledger_scale -- --ignored --nocapture
```
