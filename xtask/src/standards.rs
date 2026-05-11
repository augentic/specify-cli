//! Standards-check engine.
//!
//! Each predicate counts a violation per Rust source file. Per-file
//! baselines live in `scripts/standards-allowlist.toml`. A file's live
//! count must not exceed its baseline; the baseline defaults to 0 when
//! omitted (i.e. new files start clean).
//!
//! Three run modes:
//!
//! - [`Mode::Check`] — default; fail when any live count exceeds its baseline.
//! - [`Mode::Tighten`] — rewrite the allowlist so each baseline equals today's
//!   actual count. Use after a migration shrinks a file's count to lock it in.
//! - [`Mode::CheckTightenable`] — fail when any baseline could be tightened.
//!   CI runs this so unrelated PRs cannot mask incidental progress.
//!
//! Predicates (each implemented in [`ast_predicates`] or [`regex_predicates`]):
//!
//! - `inline-dtos` — `#[derive(Serialize)]` declared inside any
//!   `Block` (function bodies, match arms, etc.). AST-based; reliably
//!   sees DTOs defined in match arms that the prior bash regex missed.
//! - `format-match-dispatch` — `match … format { Json => … }`. Should
//!   route through `Render::render_text` + `emit` instead.
//! - `rfc-numbers-in-code` — `RFC[- ]?\d+` outside `tests/`,
//!   `DECISIONS.md`, and `rfcs/`.
//! - `ritual-doc-paragraphs` — the boilerplate `Returns an error if
//!   the operation fails.` doc paragraph.
//! - `no-op-forwarders` — `let _ = cli.<flag>;` style ignores of
//!   parsed-but-unused flags.
//! - `error-envelope-inlined` — `output::ErrorBody { … }` /
//!   `output::ValidationErrBody { … }` constructed outside
//!   `src/output.rs`. Hand-rolled error envelopes bypass the
//!   `report` path; nobody outside `output.rs` should be
//!   building the envelope DTO directly.
//! - `path-helper-inlined` — `fn specify_dir|plan_path|
//!   change_brief_path|archive_dir` declared outside `crates/config/`.
//!   Path helpers live in `specify-config`; command modules call them,
//!   they do not redefine them. Thin facade methods that take `&self`
//!   are excluded by the regex shape (the predicate targets free
//!   functions, not delegating accessors).
//! - `direct-fs-write` — direct `fs::write` / `std::fs::write` in
//!   non-test Rust. Managed state should go through the atomic helpers
//!   unless a file has an explicit baseline.
//! - `stale-cli-vocab` — legacy CLI vocabulary in non-test Rust:
//!   `initiative`, `initiative.md`, retired top-level `specify plan`,
//!   `specify merge`, or `specify validate` strings.
//! - `module-line-count` — non-test Rust source file length in lines.
//!   Default cap is 400; explicit per-file baselines grandfather oversized
//!   files until they are split.
//! - `result-cliresult-default` — free function returning
//!   `Result<Exit>` outside `src/commands.rs`. The dispatcher
//!   legitimately accepts both `Result<()>` and `Result<Exit>`
//!   shapes; new handlers should default to `Result<()>` and let
//!   success collapse to `Exit::Success`. Genuine non-success-exit
//!   handlers (typed `*ErrBody` paths) are grandfathered via
//!   per-file baselines.
//! - `verbose-doc-paragraphs` — a `///` doc paragraph longer than 8
//!   consecutive lines on a `pub fn|struct|enum|const|type`. Long
//!   prose blocks belong in `rfcs/` or `DECISIONS.md`; the doc comment
//!   on a public item should fit on a screen. `pub trait` is exempt
//!   (the contract often warrants the long form).
//! - `cli-help-shape` — clap-derive `///` doc lines longer than 80
//!   characters in `src/cli.rs` and `src/commands/**/cli.rs`. Help
//!   output is operator-facing and must wrap cleanly in a terminal.

mod allowlist;
mod ast_predicates;
mod regex_predicates;
mod report;
mod types;

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
