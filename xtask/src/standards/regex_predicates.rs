//! Regex-based predicates and the comment-stripping helper they share.
//! Each `pub(super)` function computes one predicate's live count for a
//! single Rust source file.

use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;

const VERBOSE_DOC_CAP: usize = 8;
const CLI_HELP_LINE_CAP: usize = 80;

fn count_regex(re: &Regex, text: &str) -> u32 {
    u32::try_from(re.find_iter(text).count()).unwrap_or(u32::MAX)
}

// ---------------------------------------------------------------------
// Comment stripping (used by every predicate that must ignore prose).

/// Replace `// …`, `/* … */`, and string-literal contents with whitespace
/// so the textual predicates do not match on commentary or doc copy.
pub(super) fn strip_comments(source: &str) -> String {
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
// format-match-dispatch: hand-rolled `match … format { Json => … }`.

static FORMAT_MATCH_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"match\s+(?:ctx\.|self\.)?format\s*\{").expect("static regex"));

pub(super) fn format_match_dispatch(stripped: &str) -> u32 {
    count_regex(&FORMAT_MATCH_RE, stripped)
}

// ---------------------------------------------------------------------
// rfc-numbers-in-code: `RFC[- ]?\d+` outside `tests/`, `DECISIONS.md`,
// and `rfcs/`. Scans the raw source so comments still count.

static RFC_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"RFC[- ]?\d+").expect("static regex"));

pub(super) fn rfc_numbers_in_code(source: &str) -> u32 {
    count_regex(&RFC_RE, source)
}

// ---------------------------------------------------------------------
// ritual-doc-paragraphs: boilerplate `Returns an error if the operation
// fails.` doc paragraphs.

static RITUAL_DOC_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"///\s*Returns an error if the operation fails\.").expect("static regex")
});

pub(super) fn ritual_doc_paragraphs(source: &str) -> u32 {
    count_regex(&RITUAL_DOC_RE, source)
}

// ---------------------------------------------------------------------
// no-op-forwarders: `let _ = cli.<flag>;` — a parsed-but-unused CLI flag.

static NO_OP_FORWARDER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"let\s+_\s*=\s*cli\.\w+\s*;").expect("static regex"));

pub(super) fn no_op_forwarders(stripped: &str) -> u32 {
    count_regex(&NO_OP_FORWARDER_RE, stripped)
}

// ---------------------------------------------------------------------
// direct-fs-write: direct `fs::write` / `std::fs::write` in non-test Rust.

static DIRECT_FS_WRITE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(?:std::)?fs::write\s*\(").expect("static regex"));

pub(super) fn direct_fs_write(stripped: &str) -> u32 {
    count_regex(&DIRECT_FS_WRITE_RE, stripped)
}

// ---------------------------------------------------------------------
// error-envelope-inlined: `output::ErrorBody { … }` /
// `output::ValidationErrBody { … }` constructed outside `src/output.rs`.
// Hand-rolled error envelopes bypass the `report` path.

static ERROR_ENVELOPE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"output::ErrorBody\s*\{|output::ValidationErrBody\s*\{").expect("static regex")
});

pub(super) fn error_envelope_inlined(path: &Path, stripped: &str) -> u32 {
    if is_output_module(path) { 0 } else { count_regex(&ERROR_ENVELOPE_RE, stripped) }
}

fn is_output_module(path: &Path) -> bool {
    let normalized = path.to_string_lossy().replace('\\', "/");
    normalized.ends_with("src/output.rs")
}

// ---------------------------------------------------------------------
// path-helper-inlined: `fn specify_dir|plan_path|change_brief_path|archive_dir`
// declared outside `crates/config/`. Path helpers live in
// `specify-config` (`Layout<'a>` inherent methods on the typed
// `.specify/` view); command modules call them through
// `dir.layout().plan_path()` and friends, they do not redefine them.
// The regex requires the function's first argument to start with an
// identifier (e.g. `project_dir: &Path`) rather than `&self`, so the
// `Layout` inherent methods inside `crates/config/` and any thin facade
// methods on `Ctx` are not flagged. The Rust `regex` crate has no
// lookarounds, so the negative ("not a self method") is encoded as a
// positive ("first arg is a normal identifier").

static PATH_HELPER_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"fn\s+(specify_dir|plan_path|change_brief_path|archive_dir)\s*\(\s*[A-Za-z_]")
        .expect("static regex")
});

pub(super) fn path_helper_inlined(path: &Path, stripped: &str) -> u32 {
    if is_config_crate(path) { 0 } else { count_regex(&PATH_HELPER_RE, stripped) }
}

fn is_config_crate(path: &Path) -> bool {
    let normalized = path.to_string_lossy().replace('\\', "/");
    normalized.contains("crates/config/")
}

// ---------------------------------------------------------------------
// stale-cli-vocab: legacy CLI vocabulary in non-test Rust.

static STALE_CLI_VOCAB_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\binitiative(?:\.md|_name)?\b|\bspecify (?:plan|merge|validate)\b")
        .expect("static regex")
});

pub(super) fn stale_cli_vocab(path: &Path, source: &str) -> u32 {
    let normalized = path.to_string_lossy().replace('\\', "/");
    if normalized.contains("/tests/") || normalized.ends_with("/tests.rs") {
        0
    } else {
        count_regex(&STALE_CLI_VOCAB_RE, source)
    }
}

// ---------------------------------------------------------------------
// result-cliresult-default: free `fn ... -> Result<Exit>` outside
// `src/commands.rs`. New handlers should default to `Result<()>` and
// let the dispatcher collapse the success path. The dispatcher
// legitimately keeps `Result<Exit>` to pass through both shapes; legacy
// handlers that surface a non-success exit via a typed `*ErrBody` are
// grandfathered via per-file baselines until they migrate.

static RESULT_CLIRESULT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\bfn\s+[a-z_][a-z0-9_]*\s*\([^)]*\)\s*->\s*Result<Exit\b").expect("static regex")
});

pub(super) fn result_cliresult_default(path: &Path, stripped: &str) -> u32 {
    if is_dispatcher_root(path) { 0 } else { count_regex(&RESULT_CLIRESULT_RE, stripped) }
}

fn is_dispatcher_root(path: &Path) -> bool {
    let normalized = path.to_string_lossy().replace('\\', "/");
    normalized.ends_with("src/commands.rs")
}

// ---------------------------------------------------------------------
// verbose-doc-paragraphs: a `///` doc paragraph longer than 8
// consecutive non-blank lines on a `pub fn|struct|enum|const|type`.
// `pub trait` is exempt — the contract often warrants the long form.
//
// The scanner walks each `pub` item line, skips backwards over
// attribute lines (`#[…]`), then counts consecutive non-blank `///`
// lines until it hits a blank `///` separator (which delimits a
// paragraph) or any non-doc line. The pre-item paragraph length is
// the metric; one violation per item.

pub(super) fn verbose_doc_paragraphs(source: &str) -> u32 {
    let lines: Vec<&str> = source.lines().collect();
    let mut hits = 0u32;
    for (idx, line) in lines.iter().enumerate() {
        if !is_pub_non_trait_item(line) {
            continue;
        }
        let paragraph_len = trailing_doc_paragraph_len(&lines, idx);
        if paragraph_len > VERBOSE_DOC_CAP {
            hits = hits.saturating_add(1);
        }
    }
    hits
}

fn is_pub_non_trait_item(line: &str) -> bool {
    let trimmed = line.trim_start();
    let rest = trimmed.strip_prefix("pub ").unwrap_or("");
    matches!(rest.split_whitespace().next(), Some("fn" | "struct" | "enum" | "const" | "type"))
}

/// Length of the doc paragraph immediately preceding `idx`. Walks
/// upward, skipping attribute lines, then counts consecutive non-blank
/// `///` lines. Stops at the first blank `///` (paragraph separator)
/// or any non-doc line. Returns 0 when there is no doc paragraph.
fn trailing_doc_paragraph_len(lines: &[&str], idx: usize) -> usize {
    let mut cursor = idx;
    while cursor > 0 {
        cursor -= 1;
        let trimmed = lines[cursor].trim_start();
        if trimmed.starts_with("#[") || trimmed.starts_with("#![") {
            continue;
        }
        if trimmed.is_empty() {
            return 0;
        }
        break;
    }
    if cursor == 0 && !is_non_blank_doc(lines[cursor]) {
        return 0;
    }
    let mut len = 0usize;
    let mut walk = cursor + 1;
    while walk > 0 {
        walk -= 1;
        let line = lines[walk];
        if is_non_blank_doc(line) {
            len += 1;
        } else {
            break;
        }
        if walk == 0 {
            break;
        }
    }
    len
}

fn is_non_blank_doc(line: &str) -> bool {
    let trimmed = line.trim_start();
    let Some(rest) = trimmed.strip_prefix("///") else {
        return false;
    };
    !rest.trim().is_empty()
}

// ---------------------------------------------------------------------
// cli-help-shape: clap-derive `///` doc lines longer than 80 characters
// in `src/cli.rs` and `src/commands/**/cli.rs`. Help output is
// operator-facing and wraps poorly past 80 columns. Capital-letter and
// verb-shape checks are punted (verb detection is expensive).

pub(super) fn cli_help_shape(path: &Path, source: &str) -> u32 {
    if !is_clap_cli_file(path) {
        return 0;
    }
    let mut hits = 0u32;
    for line in source.lines() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with("///") {
            continue;
        }
        if line.chars().count() > CLI_HELP_LINE_CAP {
            hits = hits.saturating_add(1);
        }
    }
    hits
}

fn is_clap_cli_file(path: &Path) -> bool {
    let normalized = path.to_string_lossy().replace('\\', "/");
    normalized.ends_with("src/cli.rs")
        || (normalized.contains("src/commands/") && normalized.ends_with("/cli.rs"))
}

// ---------------------------------------------------------------------
// module-line-count: textual LoC. Lives here so every predicate is in
// one of the two predicate modules; the actual computation is just a
// line count and does not need a regex.

pub(super) fn module_line_count(source: &str) -> u32 {
    u32::try_from(source.lines().count()).unwrap_or(u32::MAX)
}
