//! Standards-check engine. Each predicate counts violations per Rust
//! source file against the per-file baselines in
//! `scripts/standards-allowlist.toml`; live counts must not exceed them.

mod allowlist;
mod ast_predicates;
mod crate_root_prose;
mod display_serde_mirror;
mod regex_predicates;
mod report;
mod types;
mod unit_test_serde_roundtrip;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use walkdir::WalkDir;

use self::allowlist::{ALLOWLIST, Allowlist};
use self::report::Report;
use self::types::{Counts, FileBaseline};

/// How `run` interprets the result.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Mode {
    /// Standard CI check — fail when current > baseline.
    Check,
    /// Rewrite the allowlist to lock in today's actual counts.
    Tighten,
    /// Fail when any baseline could be lowered without code changes.
    CheckTightenable,
}

/// Run every predicate against `root` and act per `mode`. Returns
/// `Ok(true)` on success, `Ok(false)` on failure, `Err(_)` on I/O / parse.
pub fn run(root: &Path, mode: Mode) -> std::io::Result<bool> {
    let allowlist_path = root.join(ALLOWLIST);
    let allowlist = allowlist::load(&allowlist_path)?;
    let files = rust_files(root);

    let mut report = Report::default();
    let mut current_counts: BTreeMap<String, FileBaseline> = BTreeMap::new();
    for path in &files {
        let rel = path.strip_prefix(root).unwrap_or(path);
        let rel_str = rel.to_string_lossy().into_owned();
        let source = fs::read_to_string(path)?;
        let counts = count_one(path, &source);
        let baseline = allowlist.for_file(&rel_str);
        report.merge(&rel_str, &counts, &baseline);
        current_counts.insert(rel_str, counts.into_baseline());
    }

    match mode {
        Mode::Check => {
            report.print();
            Ok(report.passed)
        }
        Mode::Tighten => tighten(&allowlist_path, &allowlist, &current_counts),
        Mode::CheckTightenable => Ok(check_tightenable(&allowlist, &current_counts)),
    }
}

fn tighten(
    path: &Path, allowlist: &Allowlist, current: &BTreeMap<String, FileBaseline>,
) -> std::io::Result<bool> {
    let rewrites = allowlist::compute_rewrites(allowlist, current);
    if rewrites.is_empty() {
        println!("standards-check: nothing to tighten.");
        return Ok(true);
    }
    allowlist::write(path, current)?;
    println!("standards-check: tightened {} entr{}", rewrites.len(), pluralise(rewrites.len()));
    for line in &rewrites {
        println!("  {line}");
    }
    Ok(true)
}

fn check_tightenable(allowlist: &Allowlist, current: &BTreeMap<String, FileBaseline>) -> bool {
    let rewrites = allowlist::compute_rewrites(allowlist, current);
    if rewrites.is_empty() {
        println!("standards-check: allowlist is tight.");
        return true;
    }
    println!(
        "standards-check: {} allowlist entr{} can be tightened. Run \
        `cargo run -p xtask -- standards-check --tighten` and commit \
        the updated `{ALLOWLIST}`.",
        rewrites.len(),
        pluralise(rewrites.len())
    );
    for line in &rewrites {
        println!("  {line}");
    }
    false
}

const fn pluralise(n: usize) -> &'static str {
    if n == 1 { "y" } else { "ies" }
}

fn count_one(path: &Path, source: &str) -> Counts {
    let stripped = regex_predicates::strip_comments(source);
    Counts {
        inline_dtos: ast_predicates::count_inline_dtos(source),
        format_match_dispatch: regex_predicates::format_match_dispatch(&stripped),
        rfc_numbers_in_code: regex_predicates::rfc_numbers_in_code(source),
        ritual_doc_paragraphs: regex_predicates::ritual_doc_paragraphs(source),
        no_op_forwarders: regex_predicates::no_op_forwarders(&stripped),
        error_envelope_inlined: regex_predicates::error_envelope_inlined(path, &stripped),
        path_helper_inlined: regex_predicates::path_helper_inlined(path, &stripped),
        direct_fs_write: regex_predicates::direct_fs_write(&stripped),
        stale_cli_vocab: regex_predicates::stale_cli_vocab(path, source),
        module_line_count: regex_predicates::module_line_count(source),
        result_cliresult_default: regex_predicates::result_cliresult_default(path, &stripped),
        verbose_doc_paragraphs: regex_predicates::verbose_doc_paragraphs(source),
        cli_help_shape: regex_predicates::cli_help_shape(path, source),
        display_serde_mirror: display_serde_mirror::count(source),
        crate_root_prose: crate_root_prose::count(path, source),
        unit_test_serde_roundtrip: unit_test_serde_roundtrip::count(source),
        mod_rs_forbidden: u32::from(path.file_name().is_some_and(|n| n == "mod.rs")),
    }
}

fn rust_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for parent in ["src", "crates"] {
        let dir = root.join(parent);
        if !dir.exists() {
            continue;
        }
        for entry in WalkDir::new(&dir).into_iter().flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            if path.extension().is_none_or(|e| e != "rs") {
                continue;
            }
            // Skip integration test dirs and generated/target output.
            let rel = path.strip_prefix(root).unwrap_or(path);
            let rel_str = rel.to_string_lossy();
            if rel_str.starts_with("target/") || rel_str.contains("/target/") {
                continue;
            }
            if rel_str.contains("/tests/") || rel_str.ends_with("/tests.rs") {
                // Tests are exempt from the standards-check (per
                // existing AGENTS.md guidance).
                continue;
            }
            out.push(path.to_path_buf());
        }
    }
    out.sort();
    out
}
