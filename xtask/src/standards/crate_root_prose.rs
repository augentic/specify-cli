//! `crate-root-prose`: a `lib.rs` or `main.rs` whose leading `//!`
//! preamble exceeds [`CAP`] non-blank lines (skipping blanks and inner
//! `#![...]` attributes). One violation per offending file.

use std::path::Path;

const CAP: usize = 30;

/// Returns 1 when `path` is a crate root (`lib.rs` or `main.rs`) and
/// the leading `//!` doc paragraph exceeds [`CAP`] non-blank lines, 0
/// otherwise.
pub(super) fn count(path: &Path, source: &str) -> u32 {
    if !is_crate_root(path) {
        return 0;
    }
    u32::from(leading_doc_lines(source) > CAP)
}

fn is_crate_root(path: &Path) -> bool {
    let name = path.file_name().and_then(|s| s.to_str());
    matches!(name, Some("lib.rs" | "main.rs"))
}

/// Count consecutive `//!` doc lines at the top of `source`, skipping
/// blank lines and `#![...]` inner attributes that may be interleaved.
/// Stops at the first non-doc, non-blank, non-attribute line.
fn leading_doc_lines(source: &str) -> usize {
    let mut count = 0usize;
    for line in source.lines() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with("#![") {
            continue;
        }
        if trimmed.starts_with("//!") {
            count += 1;
            continue;
        }
        break;
    }
    count
}
