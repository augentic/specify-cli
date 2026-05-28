//! `specify-ignore` directive extractor per RFC-33a §"Ignore
//! directives" (D3).
//!
//! Recognises one grammar — `specify-ignore: <RULE-ID> — <rationale>`
//! — inside the closed comment-style list:
//!
//! - C-family (`rust`, `swift`, `kotlin`, `typescript`, `javascript`):
//!   `// specify-ignore: …` and `/* specify-ignore: … */`.
//! - Hash (`python`, `yaml`, `toml`): `# specify-ignore: …`.
//! - HTML (`markdown`): `<!-- specify-ignore: … -->`.
//! - SQL/Lua (`sql`): `-- specify-ignore: …`.
//!
//! Files whose inferred language sits outside the list are skipped
//! without falling back to heuristics; binary files are skipped too.
//!
//! Malformed directives (missing rationale, no separator, or empty
//! rationale) are still emitted with `rationale = None` so the
//! directive-validation pass per RFC-33a §"Implementation plan" step
//! 4 can synthesise `UNI-022`. The length-check for rationales
//! shorter than 16 characters is also the validation pass's job; this
//! extractor captures whatever rationale text is present.
//!
//! `target_line` follows the §"Ignore directives" scope rules:
//!
//! - Block-leading directives (the comment is the first non-whitespace
//!   on the line) target the next non-blank, non-comment line.
//!   Multiple consecutive block-leading directives compose: each
//!   points at the same eventual code line.
//! - Inline trailing directives (the comment follows code on the same
//!   line) target the line they live on.
//! - When no following non-blank, non-comment line exists,
//!   `target_line` is set to one past the file's last line so the
//!   validation pass treats the directive as orphaned.

mod parse;

use super::files::DiscoveredFile;
use crate::lint::{FileKind, IgnoreDirective};

/// Closed set of comment families the extractor recognises per
/// RFC-33a §"Ignore directives".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Family {
    /// `// …` and `/* … */`.
    C,
    /// `# …`.
    Hash,
    /// `<!-- … -->`.
    Html,
    /// `-- …`.
    SqlLua,
}

/// Pick a comment family from the indexer-inferred language token,
/// or `None` for languages outside the closed RFC-33a list.
fn family_for(language: &str) -> Option<Family> {
    match language {
        "rust" | "swift" | "kotlin" | "typescript" | "javascript" => Some(Family::C),
        "python" | "yaml" | "toml" => Some(Family::Hash),
        "markdown" => Some(Family::Html),
        "sql" => Some(Family::SqlLua),
        _ => None,
    }
}

/// Extract every `specify-ignore` directive from `file`. Returns an
/// empty vector when the file is binary or its language sits outside
/// the closed comment-style list.
#[must_use]
pub fn extract(file: &DiscoveredFile) -> Vec<IgnoreDirective> {
    if file.kind != FileKind::Text {
        return Vec::new();
    }
    let Some(family) = file.language.as_deref().and_then(family_for) else {
        return Vec::new();
    };

    let text = file.text();
    let lines: Vec<&str> = text.split('\n').collect();
    let total_lines = u32::try_from(lines.len()).unwrap_or(u32::MAX);

    let parsed: Vec<Option<parse::Parsed>> =
        lines.iter().map(|line| parse::parse_line(line, family)).collect();

    let mut out: Vec<IgnoreDirective> = Vec::new();
    for (idx, slot) in parsed.iter().enumerate() {
        let Some(p) = slot else { continue };
        let line_no = u32::try_from(idx + 1).unwrap_or(u32::MAX);
        let target_line =
            if p.is_trailing { line_no } else { find_target(&lines, idx, family, total_lines) };
        out.push(IgnoreDirective {
            path: file.relative.clone(),
            line: line_no,
            rule_id: p.rule_id.clone(),
            rationale: p.rationale.clone(),
            target_line,
            raw: p.raw.clone(),
        });
    }
    out
}

/// Walk forward from the line at `idx` and return the 1-based line
/// of the next non-blank, non-comment-only line. When no such line
/// exists, return `total_lines + 1` so the validation pass can
/// detect that the directive sits past EOF.
fn find_target(lines: &[&str], idx: usize, family: Family, total_lines: u32) -> u32 {
    for (offset, line) in lines.iter().enumerate().skip(idx + 1) {
        if line.trim().is_empty() {
            continue;
        }
        if is_comment_only(line, family) {
            continue;
        }
        return u32::try_from(offset + 1).unwrap_or(u32::MAX);
    }
    total_lines.saturating_add(1)
}

/// Per-family leading-delimiter check used to skip comment-only
/// lines while walking forward to the directive's target line.
fn is_comment_only(line: &str, family: Family) -> bool {
    let trimmed = line.trim_start();
    if trimmed.is_empty() {
        return false;
    }
    match family {
        Family::C => trimmed.starts_with("//") || trimmed.starts_with("/*"),
        Family::Hash => trimmed.starts_with('#'),
        Family::Html => trimmed.starts_with("<!--"),
        Family::SqlLua => trimmed.starts_with("--"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(relative: &str, language: &str, body: &str) -> DiscoveredFile {
        DiscoveredFile {
            relative: relative.into(),
            kind: FileKind::Text,
            language: Some(language.into()),
            bytes: Some(body.as_bytes().to_vec()),
        }
    }

    fn only(mut directives: Vec<IgnoreDirective>) -> IgnoreDirective {
        assert_eq!(directives.len(), 1, "expected one directive, got {directives:?}");
        directives.pop().expect("checked non-empty")
    }

    #[test]
    fn rust_double_slash_directive_is_recognised() {
        let f = file(
            "src/lib.rs",
            "rust",
            "// specify-ignore: UNI-014 — Hardcoded for the demo fixture\nlet x = 1;\n",
        );
        let d = only(extract(&f));
        assert_eq!(d.line, 1);
        assert_eq!(d.target_line, 2);
        assert_eq!(d.rule_id, "UNI-014");
        assert_eq!(d.rationale.as_deref(), Some("Hardcoded for the demo fixture"));
        assert!(d.raw.starts_with("// specify-ignore:"));
    }

    #[test]
    fn python_hash_directive_is_recognised() {
        let f = file(
            "scripts/run.py",
            "python",
            "# specify-ignore: UNI-014 -- demo rationale that is long\nresult = compute()\n",
        );
        let d = only(extract(&f));
        assert_eq!(d.line, 1);
        assert_eq!(d.target_line, 2);
        assert_eq!(d.rationale.as_deref(), Some("demo rationale that is long"));
    }

    #[test]
    fn markdown_html_directive_is_recognised() {
        let f = file(
            "docs/note.md",
            "markdown",
            "<!-- specify-ignore: UNI-014 — explained in commit message body -->\n# Heading\n",
        );
        let d = only(extract(&f));
        assert_eq!(d.line, 1);
        assert_eq!(d.target_line, 2);
        assert!(d.raw.ends_with("-->"));
    }

    #[test]
    fn sql_double_dash_directive_is_recognised() {
        let f = file(
            "migrations/001.sql",
            "sql",
            "-- specify-ignore: UNI-014 — legacy schema kept verbatim\nSELECT 1;\n",
        );
        let d = only(extract(&f));
        assert_eq!(d.line, 1);
        assert_eq!(d.target_line, 2);
    }

    #[test]
    fn em_dash_and_double_hyphen_separators_both_work() {
        let em = file(
            "a.rs",
            "rust",
            "// specify-ignore: UNI-001 — a long enough rationale\nfn f() {}\n",
        );
        let dd = file(
            "b.rs",
            "rust",
            "// specify-ignore: UNI-001 -- a long enough rationale\nfn f() {}\n",
        );
        assert_eq!(only(extract(&em)).rationale.as_deref(), Some("a long enough rationale"));
        assert_eq!(only(extract(&dd)).rationale.as_deref(), Some("a long enough rationale"));
    }

    #[test]
    fn inline_trailing_directive_targets_its_own_line() {
        let f = file(
            "src/lib.rs",
            "rust",
            "let x = foo(); // specify-ignore: UNI-014 — inline trailing reason here\n",
        );
        let d = only(extract(&f));
        assert_eq!(d.line, 1);
        assert_eq!(d.target_line, 1);
        assert!(d.raw.starts_with("// specify-ignore:"));
    }

    #[test]
    fn blank_lines_between_directive_and_target_are_skipped() {
        let f = file(
            "src/lib.rs",
            "rust",
            "// specify-ignore: UNI-014 — blank line skip rationale here\n\n\nlet x = 1;\n",
        );
        let d = only(extract(&f));
        assert_eq!(d.line, 1);
        assert_eq!(d.target_line, 4);
    }

    #[test]
    fn consecutive_directives_compose_to_same_target() {
        let f = file(
            "src/lib.rs",
            "rust",
            "// specify-ignore: UNI-014 — first rationale that is long\n\
             // specify-ignore: UNI-015 — second rationale that is long\n\
             let x = 1;\n",
        );
        let directives = extract(&f);
        assert_eq!(directives.len(), 2);
        assert_eq!(directives[0].rule_id, "UNI-014");
        assert_eq!(directives[1].rule_id, "UNI-015");
        assert_eq!(directives[0].target_line, 3);
        assert_eq!(directives[1].target_line, 3);
    }

    #[test]
    fn missing_rationale_is_captured_with_none() {
        let f = file("src/lib.rs", "rust", "// specify-ignore: UNI-014\nlet x = 1;\n");
        let d = only(extract(&f));
        assert_eq!(d.rule_id, "UNI-014");
        assert!(d.rationale.is_none());
    }

    #[test]
    fn short_rationale_is_captured_verbatim() {
        let f = file("src/lib.rs", "rust", "// specify-ignore: UNI-014 — short\nlet x = 1;\n");
        let d = only(extract(&f));
        assert_eq!(d.rule_id, "UNI-014");
        assert_eq!(d.rationale.as_deref(), Some("short"));
    }

    #[test]
    fn c_family_block_comment_directive_is_recognised() {
        let f = file(
            "src/lib.rs",
            "rust",
            "let x = /* specify-ignore: UNI-014 — block comment rationale */ 1;\n",
        );
        let d = only(extract(&f));
        assert_eq!(d.line, 1);
        assert_eq!(d.target_line, 1);
        assert_eq!(d.rule_id, "UNI-014");
        assert_eq!(d.rationale.as_deref(), Some("block comment rationale"));
        assert!(d.raw.starts_with("/* specify-ignore:"));
        assert!(d.raw.ends_with("*/"));
    }

    #[test]
    fn directive_token_inside_string_literal_is_ignored() {
        let f = file(
            "src/lib.rs",
            "rust",
            "let s = \"specify-ignore: UNI-014 — looks like a directive\";\nlet y = 2;\n",
        );
        assert!(extract(&f).is_empty(), "string-literal token must not match");
    }

    #[test]
    fn target_line_past_eof_when_directive_is_terminal() {
        let f =
            file("src/lib.rs", "rust", "// specify-ignore: UNI-014 — terminal directive only\n");
        let d = only(extract(&f));
        assert_eq!(d.line, 1);
        // `split('\n')` yields two segments here ("" trailing) so
        // total_lines == 2 and target_line lands at 3 (past EOF).
        assert_eq!(d.target_line, 3);
    }

    #[test]
    fn languages_outside_closed_list_are_skipped() {
        let f =
            file("data.json", "json", "{\"//\": \"specify-ignore: UNI-014 — no comments here\"}\n");
        assert!(extract(&f).is_empty());
    }

    #[test]
    fn binary_files_are_skipped() {
        let f = DiscoveredFile {
            relative: "blob.rs".into(),
            kind: FileKind::Binary,
            language: Some("rust".into()),
            bytes: None,
        };
        assert!(extract(&f).is_empty());
    }
}
