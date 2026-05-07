//! Resolve global / project ledger directories.
//!
//! Mirrors v2's resolution: global ledger lives at `~/.claude/ledger/`,
//! project ledger lives at `<project>/.claude/cortex/ledger/`. v2 used
//! `<project>/.claude/cache/ledger/` — v3 moves to `cortex/ledger/` so the
//! cache directory is for ephemeral/regenerable state only.

use std::path::{Path, PathBuf};

const PROJECT_LEDGER_RELATIVE: &str = ".claude/cortex/ledger";
const GLOBAL_LEDGER_RELATIVE: &str = ".claude/ledger";

pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

pub fn global_ledger_path() -> Option<PathBuf> {
    home_dir().map(|h| h.join(GLOBAL_LEDGER_RELATIVE))
}

/// Resolve the project ledger path:
/// - if `project_dir` is provided, use that as the project root;
/// - otherwise, the current working directory.
pub fn project_ledger_path(project_dir: Option<&Path>) -> std::io::Result<PathBuf> {
    let root = match project_dir {
        Some(p) => p.to_path_buf(),
        None => std::env::current_dir()?,
    };
    Ok(root.join(PROJECT_LEDGER_RELATIVE))
}

/// Project-specific ledger if `project_dir` is provided OR the global ledger
/// (per v2 semantics: `None` falls through to the global ledger; this differs
/// slightly from `project_ledger_path` which always returns a project path).
pub fn resolve_ledger_path(project_dir: Option<&Path>) -> std::io::Result<PathBuf> {
    match project_dir {
        Some(_) => project_ledger_path(project_dir),
        None => global_ledger_path()
            .ok_or_else(|| std::io::Error::other("HOME not set; cannot locate global ledger")),
    }
}
