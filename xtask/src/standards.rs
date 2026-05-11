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
//! Predicates:
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
//! - `name-suffix-duplication` — `fn foo_<module>` inside `mod
//!   <module>` (e.g. `fn show_registry` in `commands/registry.rs`).
//! - `currently-audit` — the word `Currently` in a clap-derive doc
//!   comment (`src/cli.rs` and `src/commands/**/cli.rs`). A doc that
//!   says "Currently equivalent to the default …" is the AGENTS.md
//!   `Wired-but-ignored flags` smell.
//! - `error-envelope-inlined` — `output::ErrorBody { … }` /
//!   `output::ValidationErrBody { … }` constructed outside
//!   `src/output.rs`. Hand-rolled error envelopes bypass the
//!   `report_error` path; nobody outside `output.rs` should be
//!   building the envelope DTO directly.
//! - `path-helper-inlined` — `fn specify_dir|plan_path|
//!   change_brief_path|archive_dir` declared outside `crates/config/`.
//!   Path helpers live in `specify-config`; command modules call them,
//!   they do not redefine them. Thin facade methods that take `&self`
//!   are excluded by the regex shape (the predicate targets free
//!   functions, not delegating accessors).
//! - `ok-literal-in-body` — `pub ok: bool` field on a Serialize DTO
//!   outside the two carve-outs (`crates/validate/src/contracts/envelope.rs`
//!   and `crates/validate/src/compatibility/mod.rs`). The JSON envelope
//!   encodes success-vs-failure via the presence/absence of `error:`;
//!   the redundant `ok` field was removed in CL-E3 and this predicate
//!   keeps it gone. Pragmatic regex on `pub ok: bool` rather than an
//!   AST walk — the field is always `pub` on Serialize DTOs in this
//!   workspace, so the simpler form catches every regression while
//!   missing only private `ok: bool` fields (which are not part of any
//!   wire envelope and were not flagged by CL-E3 either).
//! - `direct-fs-write` — direct `fs::write` / `std::fs::write` in
//!   non-test Rust. Managed state should go through the atomic helpers
//!   unless a file has an explicit baseline.
//! - `stale-cli-vocab` — legacy CLI vocabulary in non-test Rust:
//!   `initiative`, `initiative.md`, retired top-level `specify plan`,
//!   `specify merge`, or `specify validate` strings.
//! - `module-line-count` — non-test Rust source file length in lines.
//!   Default cap is 500; explicit per-file baselines grandfather oversized
//!   files until they are split.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use regex::Regex;
use serde::Deserialize;
use syn::visit::Visit;
use walkdir::WalkDir;

const ALLOWLIST: &str = "scripts/standards-allowlist.toml";
const DEFAULT_LINE_CAP: u32 = 500;

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
    let allowlist = load_allowlist(&allowlist_path)?;
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
        Mode::Tighten => {
            let rewrites = compute_rewrites(&allowlist, &current_counts);
            if rewrites.is_empty() {
                println!("standards-check: nothing to tighten.");
                return Ok(true);
            }
            write_allowlist(&allowlist_path, &current_counts)?;
            println!(
                "standards-check: tightened {} entr{}",
                rewrites.len(),
                pluralise(rewrites.len())
            );
            for line in &rewrites {
                println!("  {line}");
            }
            Ok(true)
        }
        Mode::CheckTightenable => {
            let rewrites = compute_rewrites(&allowlist, &current_counts);
            if rewrites.is_empty() {
                println!("standards-check: allowlist is tight.");
                return Ok(true);
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
            Ok(false)
        }
    }
}

const fn pluralise(n: usize) -> &'static str {
    if n == 1 { "y" } else { "ies" }
}

#[derive(Default, Debug)]
struct Counts {
    inline_dtos: u32,
    format_match_dispatch: u32,
    rfc_numbers_in_code: u32,
    ritual_doc_paragraphs: u32,
    no_op_forwarders: u32,
    name_suffix_duplication: u32,
    currently_audit: u32,
    error_envelope_inlined: u32,
    path_helper_inlined: u32,
    ok_literal_in_body: u32,
    direct_fs_write: u32,
    stale_cli_vocab: u32,
    module_line_count: u32,
}

impl Counts {
    fn iter(&self) -> impl Iterator<Item = (&'static str, u32)> {
        [
            ("inline-dtos", self.inline_dtos),
            ("format-match-dispatch", self.format_match_dispatch),
            ("rfc-numbers-in-code", self.rfc_numbers_in_code),
            ("ritual-doc-paragraphs", self.ritual_doc_paragraphs),
            ("no-op-forwarders", self.no_op_forwarders),
            ("name-suffix-duplication", self.name_suffix_duplication),
            ("currently-audit", self.currently_audit),
            ("error-envelope-inlined", self.error_envelope_inlined),
            ("path-helper-inlined", self.path_helper_inlined),
            ("ok-literal-in-body", self.ok_literal_in_body),
            ("direct-fs-write", self.direct_fs_write),
            ("stale-cli-vocab", self.stale_cli_vocab),
            ("module-line-count", self.module_line_count),
        ]
        .into_iter()
    }

    const fn into_baseline(self) -> FileBaseline {
        FileBaseline {
            inline_dtos: self.inline_dtos,
            format_match_dispatch: self.format_match_dispatch,
            rfc_numbers_in_code: self.rfc_numbers_in_code,
            ritual_doc_paragraphs: self.ritual_doc_paragraphs,
            no_op_forwarders: self.no_op_forwarders,
            name_suffix_duplication: self.name_suffix_duplication,
            currently_audit: self.currently_audit,
            error_envelope_inlined: self.error_envelope_inlined,
            path_helper_inlined: self.path_helper_inlined,
            ok_literal_in_body: self.ok_literal_in_body,
            direct_fs_write: self.direct_fs_write,
            stale_cli_vocab: self.stale_cli_vocab,
            module_line_count: self.module_line_count,
        }
    }
}

fn count_one(path: &Path, source: &str) -> Counts {
    let mut c = Counts::default();
    if let Ok(file) = syn::parse_file(source) {
        let mut visitor = InlineDtoVisitor { hits: 0, depth: 0 };
        visitor.visit_file(&file);
        c.inline_dtos = visitor.hits;
    }
    let stripped = strip_comments(source);
    c.format_match_dispatch = count_regex(&FORMAT_MATCH_RE, &stripped);
    c.rfc_numbers_in_code = count_regex(&RFC_RE, source);
    c.ritual_doc_paragraphs = count_regex(&RITUAL_DOC_RE, source);
    c.no_op_forwarders = count_regex(&NO_OP_FORWARDER_RE, &stripped);
    c.name_suffix_duplication = count_name_suffix(path, &stripped);
    c.currently_audit = count_currently_audit(path, source);
    c.error_envelope_inlined = count_error_envelope(path, &stripped);
    c.path_helper_inlined = count_path_helper(path, &stripped);
    c.ok_literal_in_body = count_ok_literal(path, &stripped);
    c.direct_fs_write = count_regex(&DIRECT_FS_WRITE_RE, &stripped);
    c.stale_cli_vocab = count_stale_cli_vocab(path, source);
    c.module_line_count = u32::try_from(source.lines().count()).unwrap_or(u32::MAX);
    c
}

// ---------------------------------------------------------------------
// AST: inline-dtos — Serialize derive inside any Block.

struct InlineDtoVisitor {
    hits: u32,
    depth: u32,
}

impl InlineDtoVisitor {
    fn has_serialize(attrs: &[syn::Attribute]) -> bool {
        attrs.iter().any(|a| {
            if !a.path().is_ident("derive") {
                return false;
            }
            let mut found = false;
            let _ = a.parse_nested_meta(|meta| {
                if meta.path.is_ident("Serialize") {
                    found = true;
                }
                Ok(())
            });
            found
        })
    }
}

impl<'ast> Visit<'ast> for InlineDtoVisitor {
    fn visit_block(&mut self, node: &'ast syn::Block) {
        self.depth += 1;
        syn::visit::visit_block(self, node);
        self.depth -= 1;
    }

    fn visit_item_struct(&mut self, node: &'ast syn::ItemStruct) {
        if self.depth > 0 && Self::has_serialize(&node.attrs) {
            self.hits += 1;
        }
        syn::visit::visit_item_struct(self, node);
    }

    fn visit_item_enum(&mut self, node: &'ast syn::ItemEnum) {
        if self.depth > 0 && Self::has_serialize(&node.attrs) {
            self.hits += 1;
        }
        syn::visit::visit_item_enum(self, node);
    }
}

// ---------------------------------------------------------------------
// Regex predicates.

fn count_regex(re: &Regex, text: &str) -> u32 {
    u32::try_from(re.find_iter(text).count()).unwrap_or(u32::MAX)
}

static FORMAT_MATCH_RE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
    Regex::new(r"match\s+(?:ctx\.|self\.)?format\s*\{").expect("static regex")
});

static RFC_RE: std::sync::LazyLock<Regex> =
    std::sync::LazyLock::new(|| Regex::new(r"RFC[- ]?\d+").expect("static regex"));

static RITUAL_DOC_RE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
    Regex::new(r"///\s*Returns an error if the operation fails\.").expect("static regex")
});

static NO_OP_FORWARDER_RE: std::sync::LazyLock<Regex> =
    std::sync::LazyLock::new(|| Regex::new(r"let\s+_\s*=\s*cli\.\w+\s*;").expect("static regex"));

static DIRECT_FS_WRITE_RE: std::sync::LazyLock<Regex> =
    std::sync::LazyLock::new(|| Regex::new(r"\b(?:std::)?fs::write\s*\(").expect("static regex"));

static STALE_CLI_VOCAB_RE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
    Regex::new(r"\binitiative(?:\.md|_name)?\b|\bspecify (?:plan|merge|validate)\b")
        .expect("static regex")
});

// ---------------------------------------------------------------------
// currently-audit: the word `Currently` in a clap-derive doc comment.
//
// Scoped to `src/cli.rs` and `src/commands/**/cli.rs` — the post-CL-MS-CLI
// clap-derive surface. A doc comment that says "Currently equivalent to the
// default …" is the AGENTS.md `Wired-but-ignored flags` smell: drop the
// flag from clap until the differentiated behaviour exists.
//
// Matches `Currently` (case-sensitive, word-bounded) in any doc comment
// line: `///`, `//!`, or `#[doc = "…"]`.

static CURRENTLY_DOC_RE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
    Regex::new(r"(?m)^\s*(?://[/!]|#\[doc\b).*\bCurrently\b").expect("static regex")
});

fn count_currently_audit(path: &Path, source: &str) -> u32 {
    if is_clap_cli_file(path) { count_regex(&CURRENTLY_DOC_RE, source) } else { 0 }
}

fn is_clap_cli_file(path: &Path) -> bool {
    let normalized = path.to_string_lossy().replace('\\', "/");
    normalized.ends_with("src/cli.rs")
        || (normalized.contains("src/commands/") && normalized.ends_with("/cli.rs"))
}

// ---------------------------------------------------------------------
// error-envelope-inlined: `output::ErrorBody { … }` /
// `output::ValidationErrBody { … }` constructed outside `src/output.rs`.
//
// Hand-rolled error envelopes bypass the `report_error` path.
// CL-E3 removed the last hand-rolled construction (in `src/commands/registry.rs`);
// R5 renamed the body types but kept this predicate as the only legitimate
// construction guard.

static ERROR_ENVELOPE_RE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
    Regex::new(r"output::ErrorBody\s*\{|output::ValidationErrBody\s*\{").expect("static regex")
});

fn count_error_envelope(path: &Path, stripped: &str) -> u32 {
    if is_output_module(path) { 0 } else { count_regex(&ERROR_ENVELOPE_RE, stripped) }
}

fn is_output_module(path: &Path) -> bool {
    let normalized = path.to_string_lossy().replace('\\', "/");
    normalized.ends_with("src/output.rs")
}

// ---------------------------------------------------------------------
// path-helper-inlined: `fn specify_dir|plan_path|change_brief_path|archive_dir`
// declared outside `crates/config/`.
//
// Path helpers live in `specify-config` (`Layout<'a>` inherent methods on
// the typed `.specify/` view); command modules call them through
// `dir.layout().plan_path()` and friends, they do not redefine them.
// The regex requires the function's first argument to start with an
// identifier (e.g. `project_dir: &Path`) rather than `&self`, so the
// `Layout` inherent methods inside `crates/config/` and any thin facade
// methods on `Ctx` are not flagged. The Rust `regex` crate has no
// lookarounds, so the negative ("not a self method") is encoded as a
// positive ("first arg is a normal identifier").

static PATH_HELPER_RE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
    Regex::new(r"fn\s+(specify_dir|plan_path|change_brief_path|archive_dir)\s*\(\s*[A-Za-z_]")
        .expect("static regex")
});

fn count_path_helper(path: &Path, stripped: &str) -> u32 {
    if is_config_crate(path) { 0 } else { count_regex(&PATH_HELPER_RE, stripped) }
}

fn is_config_crate(path: &Path) -> bool {
    let normalized = path.to_string_lossy().replace('\\', "/");
    normalized.contains("crates/config/")
}

// ---------------------------------------------------------------------
// ok-literal-in-body: `pub ok: bool` field outside the carve-outs.
//
// CL-E3 dropped the redundant `ok: bool` field from every success / error
// DTO under `src/commands/`. The JSON envelope encodes success-vs-failure
// via the presence/absence of `error:`; the `ok` field was duplicative
// wire noise. The two carve-outs are intentional:
//
//   - `crates/validate/src/contracts/envelope.rs` — the contract-validate
//     WASI tool envelope (schema-version 2, not routed through specify-error).
//   - `crates/validate/src/compatibility/mod.rs` — `CompatibilityReport.ok`
//     is a computed semantic flag consumed by `is_compatible()`.
//
// Pragmatic regex on `pub\s+ok:\s*bool` rather than an AST walk that
// verifies a surrounding `#[derive(Serialize)]`. Serialize DTOs in this
// workspace always declare their fields `pub`, so the simpler regex
// catches every regression while ignoring private `ok: bool` fields
// (which are not part of any wire envelope and were not flagged by CL-E3).

static OK_LITERAL_RE: std::sync::LazyLock<Regex> =
    std::sync::LazyLock::new(|| Regex::new(r"pub\s+ok:\s*bool\b").expect("static regex"));

fn count_ok_literal(path: &Path, stripped: &str) -> u32 {
    if is_ok_literal_carveout(path) { 0 } else { count_regex(&OK_LITERAL_RE, stripped) }
}

fn is_ok_literal_carveout(path: &Path) -> bool {
    let normalized = path.to_string_lossy().replace('\\', "/");
    normalized.ends_with("crates/validate/src/contracts/envelope.rs")
        || normalized.ends_with("crates/validate/src/compatibility/mod.rs")
}

fn count_stale_cli_vocab(path: &Path, source: &str) -> u32 {
    let normalized = path.to_string_lossy().replace('\\', "/");
    if normalized.contains("/tests/") || normalized.ends_with("/tests.rs") {
        0
    } else {
        count_regex(&STALE_CLI_VOCAB_RE, source)
    }
}

// ---------------------------------------------------------------------
// Name-suffix duplication: fn foo_<module> in mod <module>.

fn count_name_suffix(path: &Path, source: &str) -> u32 {
    let Some(module) = module_name(path) else {
        return 0;
    };
    let pattern = format!(r"fn\s+\w+_{module}\b");
    let re = Regex::new(&pattern).expect("dynamic regex");
    count_regex(&re, source)
}

fn module_name(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_string_lossy();
    if stem == "mod" || stem == "lib" || stem == "main" {
        path.parent()?.file_name().map(|n| n.to_string_lossy().into_owned())
    } else {
        Some(stem.into_owned())
    }
}

// ---------------------------------------------------------------------
// Comment stripping (for predicates that must ignore prose).

fn strip_comments(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut chars = source.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '/' if chars.peek() == Some(&'/') => {
                for nc in chars.by_ref() {
                    if nc == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut prev = '\0';
                for nc in chars.by_ref() {
                    if prev == '*' && nc == '/' {
                        break;
                    }
                    if nc == '\n' {
                        out.push('\n');
                    }
                    prev = nc;
                }
            }
            '"' => {
                out.push(c);
                let mut escape = false;
                for nc in chars.by_ref() {
                    out.push(nc);
                    if escape {
                        escape = false;
                    } else if nc == '\\' {
                        escape = true;
                    } else if nc == '"' {
                        break;
                    }
                }
            }
            _ => out.push(c),
        }
    }
    out
}

// ---------------------------------------------------------------------
// File discovery.

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

// ---------------------------------------------------------------------
// Allowlist (per-file TOML).

#[derive(Debug, Default, Deserialize)]
struct AllowlistRaw {
    #[serde(default)]
    file: BTreeMap<String, FileBaseline>,
}

#[derive(Debug, Default, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
struct FileBaseline {
    #[serde(default)]
    inline_dtos: u32,
    #[serde(default)]
    format_match_dispatch: u32,
    #[serde(default)]
    rfc_numbers_in_code: u32,
    #[serde(default)]
    ritual_doc_paragraphs: u32,
    #[serde(default)]
    no_op_forwarders: u32,
    #[serde(default)]
    name_suffix_duplication: u32,
    #[serde(default)]
    currently_audit: u32,
    #[serde(default)]
    error_envelope_inlined: u32,
    #[serde(default)]
    path_helper_inlined: u32,
    #[serde(default)]
    ok_literal_in_body: u32,
    #[serde(default)]
    direct_fs_write: u32,
    #[serde(default)]
    stale_cli_vocab: u32,
    #[serde(default)]
    module_line_count: u32,
}

impl FileBaseline {
    fn allowed(&self, key: &str) -> u32 {
        match key {
            "inline-dtos" => self.inline_dtos,
            "format-match-dispatch" => self.format_match_dispatch,
            "rfc-numbers-in-code" => self.rfc_numbers_in_code,
            "ritual-doc-paragraphs" => self.ritual_doc_paragraphs,
            "no-op-forwarders" => self.no_op_forwarders,
            "name-suffix-duplication" => self.name_suffix_duplication,
            "currently-audit" => self.currently_audit,
            "error-envelope-inlined" => self.error_envelope_inlined,
            "path-helper-inlined" => self.path_helper_inlined,
            "ok-literal-in-body" => self.ok_literal_in_body,
            "direct-fs-write" => self.direct_fs_write,
            "stale-cli-vocab" => self.stale_cli_vocab,
            "module-line-count" => self.module_line_count,
            _ => 0,
        }
    }

    /// Effective per-file cap. Most predicates default to 0 (new files
    /// start clean); `module-line-count` defaults to `DEFAULT_LINE_CAP`.
    fn cap(&self, key: &str) -> u32 {
        if key == "module-line-count" && self.module_line_count == 0 {
            DEFAULT_LINE_CAP
        } else {
            self.allowed(key)
        }
    }

    fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

struct Allowlist {
    files: BTreeMap<String, FileBaseline>,
}

impl Allowlist {
    fn for_file(&self, rel: &str) -> FileBaseline {
        self.files.get(rel).cloned().unwrap_or_default()
    }
}

fn load_allowlist(path: &Path) -> std::io::Result<Allowlist> {
    if !path.exists() {
        return Ok(Allowlist {
            files: BTreeMap::new(),
        });
    }
    let text = fs::read_to_string(path)?;
    let raw: AllowlistRaw = toml::from_str(&text)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err.to_string()))?;
    Ok(Allowlist { files: raw.file })
}

/// Compute human-readable diff lines for any (file, predicate) where the
/// recorded baseline differs from today's actual count. For
/// `module-line-count` we accept moves in either direction (the baseline is
/// a pure `LoC` snapshot, so growth from a routine edit should re-bake the
/// baseline). For every other predicate we only surface reductions —
/// growth is a violation, not a tightenable diff.
fn compute_rewrites(
    allowlist: &Allowlist, current: &BTreeMap<String, FileBaseline>,
) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    for (rel, baseline) in &allowlist.files {
        seen.insert(rel.as_str());
        let actual = current.get(rel).cloned().unwrap_or_default();
        if &actual == baseline {
            continue;
        }
        for (key, baseline_val) in baseline_iter(baseline) {
            let actual_val = actual.allowed(key);
            if actual_val == baseline_val {
                continue;
            }
            if key == "module-line-count" || actual_val < baseline_val {
                out.push(format!("{rel}: {key} {baseline_val} → {actual_val}"));
            }
        }
    }
    // New files that exceed DEFAULT_LINE_CAP need an explicit module-line-count
    // entry; surface them so `--tighten` stamps a baseline.
    for (rel, actual) in current {
        if seen.contains(rel.as_str()) {
            continue;
        }
        if actual.module_line_count > DEFAULT_LINE_CAP {
            out.push(format!(
                "{rel}: module-line-count {DEFAULT_LINE_CAP} → {} (new file over default cap)",
                actual.module_line_count
            ));
        }
    }
    out
}

fn baseline_iter(b: &FileBaseline) -> impl Iterator<Item = (&'static str, u32)> + '_ {
    [
        ("inline-dtos", b.inline_dtos),
        ("format-match-dispatch", b.format_match_dispatch),
        ("rfc-numbers-in-code", b.rfc_numbers_in_code),
        ("ritual-doc-paragraphs", b.ritual_doc_paragraphs),
        ("no-op-forwarders", b.no_op_forwarders),
        ("name-suffix-duplication", b.name_suffix_duplication),
        ("currently-audit", b.currently_audit),
        ("error-envelope-inlined", b.error_envelope_inlined),
        ("path-helper-inlined", b.path_helper_inlined),
        ("ok-literal-in-body", b.ok_literal_in_body),
        ("direct-fs-write", b.direct_fs_write),
        ("stale-cli-vocab", b.stale_cli_vocab),
        ("module-line-count", b.module_line_count),
    ]
    .into_iter()
}

/// Serialise `current` back to `path` as TOML, skipping rows where every
/// field equals its zero-default. Output is alphabetised by file path.
fn write_allowlist(path: &Path, current: &BTreeMap<String, FileBaseline>) -> std::io::Result<()> {
    let mut out = String::new();
    out.push_str(
        "# Per-file baselines for `cargo run -p xtask -- standards-check`.\n\
         #\n\
         # Each `[file.\"<rel-path>\"]` table caps the number of violations of each\n\
         # predicate for that file. A live count strictly greater than the\n\
         # baseline fails CI; missing predicates default to zero (new files\n\
         # start clean) except `module-line-count`, which defaults to 500.\n\
         # Reductions are encouraged in any PR that touches a file; the CI\n\
         # `--check-tightenable` mode fails when an unrelated PR could lower a\n\
         # baseline without code changes.\n\
         #\n\
         # Predicate definitions live in `xtask/src/standards.rs`. AGENTS.md\n\
         # §Mechanical enforcement explains what each predicate enforces and how\n\
         # to drive its baselines down.\n\n",
    );
    for (rel, baseline) in current {
        if baseline.is_empty() {
            continue;
        }
        let _ = writeln!(out, "[file.\"{rel}\"]");
        for (key, value) in baseline_iter(baseline) {
            if value == 0 {
                continue;
            }
            let _ = writeln!(out, "{key} = {value}");
        }
        out.push('\n');
    }
    fs::write(path, out)
}

// ---------------------------------------------------------------------
// Reporting.

#[derive(Default)]
struct Report {
    passed: bool,
    failures: Vec<String>,
    totals: BTreeMap<&'static str, u32>,
}

impl Report {
    fn merge(&mut self, rel: &str, counts: &Counts, baseline: &FileBaseline) {
        if self.failures.is_empty() {
            self.passed = true;
        }
        for (key, value) in counts.iter() {
            // module-line-count contributes to totals only as an
            // overflow indicator, not a sum (LoC totals would dwarf
            // every other predicate).
            if key != "module-line-count" {
                *self.totals.entry(key).or_insert(0) += value;
            }
            let cap = baseline.cap(key);
            if value > cap {
                self.passed = false;
                self.failures.push(format!("  FAIL {rel}: {key} {value} > baseline {cap}"));
            }
        }
    }

    fn print(&self) {
        for line in &self.failures {
            println!("{line}");
        }
        println!();
        println!("standards-check totals:");
        for (key, value) in &self.totals {
            println!("  {key}: {value}");
        }
        if self.passed {
            println!("\nstandards-check: PASS");
        } else {
            println!(
                "\nstandards-check: FAIL — reduce the offending counts or, if a hit is justified, raise the per-file baseline in {ALLOWLIST} in the same PR."
            );
        }
    }
}
