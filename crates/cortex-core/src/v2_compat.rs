//! Read-only access to v2 (Python) ledgers.
//!
//! v3 native code never writes the v2 wire format. This module exists so
//! `cortex-migrate` can validate v2 hashes/signatures against the original
//! data before transcribing it into the v3 layout.
//!
//! Hash format quirks of v2 (the reason this module exists):
//! - Block hashes are computed from `json.dumps(content, sort_keys=True)` with
//!   Python's default separators `(', ', ': ')`, not serde_json's default.
//! - Timestamps inside the hash payload are formatted via
//!   `datetime.isoformat()` which uses `+00:00` offsets, while the on-disk
//!   block file stores them with `Z` suffixes (Pydantic).

use std::collections::BTreeMap;
use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};

pub const V2_BLOCKS_DIR: &str = "blocks";
pub const V2_INDEX_FILE: &str = "index.json";
pub const V2_REINFORCEMENTS_FILE: &str = "reinforcements.json";
pub const V2_MERKLE_FILE: &str = "merkle.json";
pub const V2_IDENTITY_FILE: &str = "identity.json";

/// Parse Python's three ISO-8601 forms found in real v2 ledgers:
/// - Pydantic `Z` suffix (`2026-05-06T21:45:03.523577Z`)
/// - `datetime.isoformat()` with `+00:00` offset
/// - **Naive** (no timezone) — early v2 ledgers omit the offset because
///   `datetime.now()` (without `tz=UTC`) returns a naive datetime, and
///   Python's `fromisoformat` accepts it. We treat naive timestamps as
///   UTC since that's what v2 always intended (the early bug was fixed
///   later but pre-fix entries retain the naive form).
pub fn parse_python_iso(s: &str) -> Result<DateTime<Utc>> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Ok(dt.with_timezone(&Utc));
    }
    if let Some(prefix) = s.strip_suffix('Z') {
        if let Ok(dt) = DateTime::parse_from_rfc3339(&format!("{prefix}+00:00")) {
            return Ok(dt.with_timezone(&Utc));
        }
    }
    if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f") {
        return Ok(DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc));
    }
    Err(Error::Malformed(format!(
        "could not parse python iso datetime: {s}"
    )))
}

/// Format a UTC datetime using Python's `datetime.isoformat()` form
/// (microsecond precision, `+00:00` offset). Use this only when you know
/// the original was timezone-aware.
pub fn format_python_iso(dt: &DateTime<Utc>) -> String {
    let core = dt.format("%Y-%m-%dT%H:%M:%S%.6f").to_string();
    format!("{core}+00:00")
}

/// Convert a v2 timestamp string to its canonical hash-input form.
/// v2 hashed timestamps via `datetime.isoformat()`:
///   - For tz-aware datetimes (Pydantic `Z` form), this produces `+00:00`.
///   - For naive datetimes (early v2 ledger bug), this produces the naive
///     form unchanged.
/// So: `Z` → `+00:00`; naive → naive (passthrough).
pub fn canonical_hash_timestamp(s: &str) -> String {
    if let Some(prefix) = s.strip_suffix('Z') {
        format!("{prefix}+00:00")
    } else {
        s.to_string()
    }
}

/// Serialize a `serde_json::Value` using Python `json.dumps(sort_keys=True)`
/// default formatting. Necessary for byte-for-byte hash parity with v2.
pub fn dumps_python(value: &Value) -> String {
    let mut out = String::new();
    write_value(&mut out, value);
    out
}

fn write_value(out: &mut String, v: &Value) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Number(n) => write_number(out, n),
        Value::String(s) => write_python_string(out, s),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_value(out, item);
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (i, k) in keys.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_python_string(out, k);
                out.push_str(": ");
                write_value(out, &map[*k]);
            }
            out.push('}');
        }
    }
}

fn write_number(out: &mut String, n: &serde_json::Number) {
    use std::fmt::Write;
    if let Some(i) = n.as_i64() {
        write!(out, "{i}").unwrap();
    } else if let Some(u) = n.as_u64() {
        write!(out, "{u}").unwrap();
    } else if let Some(f) = n.as_f64() {
        let s = format!("{f}");
        if s.contains('.') || s.contains('e') || s.contains('E') {
            out.push_str(&s);
        } else {
            write!(out, "{s}.0").unwrap();
        }
    } else {
        out.push_str(&n.to_string());
    }
}

fn write_python_string(out: &mut String, s: &str) {
    use std::fmt::Write;
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{0008}' => out.push_str("\\b"),
            '\u{000C}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => write!(out, "\\u{:04x}", c as u32).unwrap(),
            c if (c as u32) < 0x7F => out.push(c),
            c => {
                let code = c as u32;
                if code <= 0xFFFF {
                    write!(out, "\\u{code:04x}").unwrap();
                } else {
                    let adjusted = code - 0x10000;
                    let high = 0xD800 + (adjusted >> 10);
                    let low = 0xDC00 + (adjusted & 0x3FF);
                    write!(out, "\\u{high:04x}\\u{low:04x}").unwrap();
                }
            }
        }
    }
    out.push('"');
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct V2Block {
    pub id: String,
    pub timestamp: String,
    pub session_id: String,
    #[serde(default)]
    pub parent_block: Option<String>,
    #[serde(default)]
    pub learnings: Vec<V2Learning>,
    #[serde(default)]
    pub author_key_id: Option<String>,
    #[serde(default)]
    pub signature: Option<String>,
    pub hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct V2Learning {
    pub id: String,
    pub category: String,
    pub content: String,
    pub confidence: f64,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub outcomes: Vec<Value>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct V2Index {
    #[serde(default)]
    pub head: Option<String>,
    #[serde(default)]
    pub blocks: Vec<V2IndexEntry>,
    #[serde(default)]
    pub merkle_root: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct V2IndexEntry {
    pub id: String,
    pub timestamp: String,
    pub hash: String,
    #[serde(default)]
    pub parent: Option<String>,
}

pub fn load_index(root: &Path) -> Result<V2Index> {
    let path = root.join(V2_INDEX_FILE);
    let bytes = std::fs::read(&path).map_err(|e| Error::io(&path, e))?;
    serde_json::from_slice(&bytes).map_err(|e| Error::json(&path, e))
}

pub fn load_block(root: &Path, block_id: &str) -> Result<V2Block> {
    let path = root.join(V2_BLOCKS_DIR).join(format!("{block_id}.json"));
    let bytes = std::fs::read(&path).map_err(|e| Error::io(&path, e))?;
    serde_json::from_slice(&bytes).map_err(|e| Error::json(&path, e))
}

pub fn list_block_ids(root: &Path) -> Result<Vec<String>> {
    let dir = root.join(V2_BLOCKS_DIR);
    let mut ids = Vec::new();
    if !dir.is_dir() {
        return Ok(ids);
    }
    for entry in std::fs::read_dir(&dir).map_err(|e| Error::io(&dir, e))? {
        let entry = entry.map_err(|e| Error::io(&dir, e))?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
            ids.push(stem.to_string());
        }
    }
    ids.sort();
    Ok(ids)
}

/// Recompute the v2 block hash from a raw `V2Block`. Matches Python:
/// `sha256(json.dumps({id, timestamp.isoformat(), session_id, parent_block,
/// learnings: [hash_dict, ...]}, sort_keys=True))`.
pub fn compute_v2_block_hash(block: &V2Block) -> Result<String> {
    // Validate parses (catches truly malformed entries) but emit the
    // canonical hash-input form: `Z`→`+00:00`, naive→naive.
    parse_python_iso(&block.timestamp)?;
    let canonical_ts = canonical_hash_timestamp(&block.timestamp);
    let mut learnings = Vec::with_capacity(block.learnings.len());
    for learning in &block.learnings {
        let outcomes_value = Value::Array(learning.outcomes.clone());
        learnings.push(json!({
            "id": learning.id,
            "category": learning.category,
            "content": learning.content,
            "confidence": learning.confidence,
            "source": learning.source,
            "outcomes": outcomes_value,
        }));
    }
    let mut top = Map::new();
    top.insert("id".to_string(), Value::String(block.id.clone()));
    top.insert("timestamp".to_string(), Value::String(canonical_ts));
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
    top.insert("learnings".to_string(), Value::Array(learnings));
    let canonical = dumps_python(&Value::Object(top));
    Ok(hex::encode(Sha256::digest(canonical.as_bytes())))
}

#[derive(Debug, Default)]
pub struct V2Verification {
    pub valid_blocks: Vec<String>,
    pub hash_mismatches: Vec<(String, String, String)>, // (block_id, stored, computed)
    pub missing_blocks: Vec<String>,
}

impl V2Verification {
    pub fn is_clean(&self) -> bool {
        self.hash_mismatches.is_empty() && self.missing_blocks.is_empty()
    }
}

/// Recompute every v2 block hash and report mismatches. Useful for sanity-
/// checking a v2 ledger before migration.
pub fn verify_v2_block_hashes(root: &Path) -> Result<V2Verification> {
    let index = load_index(root)?;
    let mut report = V2Verification::default();
    for entry in &index.blocks {
        match load_block(root, &entry.id) {
            Ok(block) => {
                let computed = compute_v2_block_hash(&block)?;
                if computed == block.hash && computed == entry.hash {
                    report.valid_blocks.push(entry.id.clone());
                } else {
                    report
                        .hash_mismatches
                        .push((entry.id.clone(), block.hash.clone(), computed));
                }
            }
            Err(Error::Io { .. }) => report.missing_blocks.push(entry.id.clone()),
            Err(e) => return Err(e),
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture_root() -> PathBuf {
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        manifest
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("tests/fixtures/v2_ledger")
    }

    #[test]
    fn parses_both_python_iso_forms() {
        let z = parse_python_iso("2026-05-06T21:45:03.523577Z").unwrap();
        let plus = parse_python_iso("2026-05-06T21:45:03.523577+00:00").unwrap();
        assert_eq!(z, plus);
    }

    #[test]
    fn dumps_python_matches_known_separators() {
        let v = json!({"b": 1, "a": [1, 2]});
        assert_eq!(dumps_python(&v), r#"{"a": [1, 2], "b": 1}"#);
    }

    #[test]
    fn validates_v2_fixture_block_hashes() {
        let root = fixture_root();
        if !root.is_dir() {
            eprintln!("skipping: fixture not present at {}", root.display());
            return;
        }
        let report = verify_v2_block_hashes(&root).unwrap();
        assert!(report.is_clean(), "v2 verification dirty: {report:?}");
        assert!(!report.valid_blocks.is_empty());
    }
}
