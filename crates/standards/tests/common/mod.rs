//! Shared filesystem helpers for `specify-standards` integration tests.
//!
//! Lives under `tests/<helper>/mod.rs` per Rust's idiom (the host
//! workspace forbids `mod.rs` everywhere else — see
//! `docs/standards/coding-standards.md` §"Module layout"). Domain
//! scaffolding (rules / hints / tool runners) lives in `eval_support`;
//! this module is the single source for generic fixture-staging helpers.

#![allow(dead_code, reason = "shared test helpers; not every integration binary uses every helper")]

use std::fs;
use std::path::Path;

/// Recursively copy `src` into `dst`, creating directories as needed.
///
/// Replaces the per-binary `copy_dir_recursive` / `copy_dir_all`
/// reimplementations across the crate's integration tests.
///
/// # Panics
///
/// Panics if `src` cannot be read or a file cannot be copied.
pub fn copy_dir(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).expect("create_dir_all dst");
    for entry in fs::read_dir(src).expect("read_dir src") {
        let entry = entry.expect("dir entry");
        let target = dst.join(entry.file_name());
        if entry.file_type().expect("file_type").is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).expect("copy");
        }
    }
}
