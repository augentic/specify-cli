//! Codex rule frontmatter parser per RFC-28 §"Codex file shape".
//!
//! Splits a codex rule markdown file into YAML frontmatter +
//! verbatim body, validates the frontmatter against the embedded
//! `schemas/codex/codex-rule.schema.json`, lifts the `snake_case`
//! authoring keys (`review_mode`, `deterministic_hints`,
//! `replaced_by`) to the kebab-case wire shape carried by
//! [`CodexRule`], and returns the typed rule with `body` set to the
//! exact post-delimiter bytes.
//!
//! # Body fidelity
//!
//! Reviewing agents consume the body verbatim as policy text, so
//! the parser preserves every newline, heading, and code fence
//! after the closing `---` delimiter. The only normalisation is
//! stripping the single newline (`\n` or `\r\n`) that immediately
//! follows the closing delimiter — without that, a file ending
//! `...---\n## Rule\n...` would yield a body starting with
//! `\n## Rule\n` instead of `## Rule\n`.
//!
//! # Schema vs wire shape
//!
//! The canonical schema at `schemas/codex/codex-rule.schema.json` is
//! the single source of truth for both the runtime resolver and
//! `specdev check`'s codex-frontmatter predicate (RFC-32 §"Eliminates
//! the vendored codex-rule schema"). It carries `snake_case` keys.
//! Validation runs against the original raw frontmatter, *before* the
//! `snake_case -> kebab-case` lift, which keeps schema semantics
//! aligned with what authors actually wrote on disk. The lift then
//! rewrites keys at every nesting level so the value can deserialize
//! cleanly into [`CodexRule`].
//!
//! # Out of scope
//!
//! No regex compilation. RFC-28 §"Deterministic hints
//! extensibility" requires the runtime resolver to never compile a
//! regex it never executes — hint execution is RFC-32 territory
//! (CH-13 +). Applicability filtering, deprecation filtering, and
//! stable ordering are CH-13 / CH-14.

use std::path::Path;
use std::{fs, io};

use serde_json::{Map as JsonMap, Value as JsonValue};
use specify_error::ValidationStatus;
use specify_schema::validate_value;

use super::CodexRule;

/// Canonical codex-rule frontmatter schema, also exposed at
/// [`specify_schema::CODEX_RULE_JSON_SCHEMA`]. Per RFC-32
/// §"Eliminates the vendored codex-rule schema", this is the single
/// source of truth — `specdev check`'s codex predicate compiles the
/// same constant via `specify-authoring`.
const CODEX_RULE_SCHEMA: &str = include_str!("../../../../schemas/codex/codex-rule.schema.json");

/// Failure modes for [`parse_codex_rule`] / [`parse_codex_rule_file`].
///
/// Mirrors the staging order of the parser: open delimiter first,
/// closing delimiter, YAML parse, schema validation, JSON
/// conversion. I/O failures from the file convenience wrapper
/// surface as [`ParseError::Io`].
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    /// File does not start with `---\n` or `---\r\n`.
    #[error("codex rule missing leading YAML frontmatter delimiter '---'")]
    MissingOpeningDelimiter,
    /// No matching `---` line is found before EOF.
    #[error("codex rule missing closing YAML frontmatter delimiter '---'")]
    MissingClosingDelimiter,
    /// `serde_saphyr` could not parse the frontmatter block as YAML.
    #[error("codex rule frontmatter YAML parse failed: {0}")]
    Yaml(String),
    /// Frontmatter parsed but failed `codex-rule.schema.json`.
    #[error("codex rule frontmatter schema validation failed: {0}")]
    Schema(String),
    /// Post-lift JSON did not deserialize into [`CodexRule`].
    #[error("codex rule frontmatter JSON conversion failed: {0}")]
    JsonConvert(String),
    /// File read failed (file convenience wrapper only).
    #[error("codex rule file read failed: {0}")]
    Io(#[from] io::Error),
}

/// Parse a codex rule markdown document into a [`CodexRule`].
///
/// The body field is set to the verbatim post-delimiter bytes; the
/// rest of the struct comes from the validated, kebab-lifted YAML
/// frontmatter. See module docs for delimiter handling, lift
/// behavior, and out-of-scope items.
///
/// # Errors
///
/// Returns the matching [`ParseError`] variant for each parsing
/// stage; see the variant docs.
pub fn parse_codex_rule(content: &str) -> Result<CodexRule, ParseError> {
    let (frontmatter, body) = split_frontmatter(content)?;

    let raw_value: JsonValue =
        serde_saphyr::from_str(frontmatter).map_err(|err| ParseError::Yaml(err.to_string()))?;

    let summaries = validate_value(
        &raw_value,
        CODEX_RULE_SCHEMA,
        "codex-rule-schema",
        "codex rule frontmatter conforms to schemas/codex/codex-rule.schema.json",
    );
    let failures: Vec<String> = summaries
        .into_iter()
        .filter(|summary| summary.status == ValidationStatus::Fail)
        .filter_map(|summary| summary.detail)
        .collect();
    if !failures.is_empty() {
        return Err(ParseError::Schema(failures.join("; ")));
    }

    let lifted = snake_to_kebab_keys(raw_value);
    let mut rule: CodexRule =
        serde_json::from_value(lifted).map_err(|err| ParseError::JsonConvert(err.to_string()))?;
    body.clone_into(&mut rule.body);
    Ok(rule)
}

/// Read `path` and parse it via [`parse_codex_rule`].
///
/// The body is preserved exactly as on disk; the parser does not
/// line-normalize. I/O failures map to [`ParseError::Io`]; every
/// other failure mode is delegated to [`parse_codex_rule`].
///
/// # Errors
///
/// Returns [`ParseError::Io`] if the file cannot be read; otherwise
/// the error from [`parse_codex_rule`].
pub fn parse_codex_rule_file(path: &Path) -> Result<CodexRule, ParseError> {
    let content = fs::read_to_string(path)?;
    parse_codex_rule(&content)
}

/// Split `content` into `(frontmatter, body)` slices.
///
/// Accepts `---\n` or `---\r\n` for the opening delimiter and
/// `\n---\n`, `\n---\r\n`, or trailing `\n---` at EOF for the
/// closing delimiter. The single newline that introduces the body
/// (the one immediately after the closing `---`) is stripped so
/// callers see body content starting at column 0; everything else
/// is verbatim.
fn split_frontmatter(content: &str) -> Result<(&str, &str), ParseError> {
    let rest = if let Some(rest) = content.strip_prefix("---\n") {
        rest
    } else if let Some(rest) = content.strip_prefix("---\r\n") {
        rest
    } else {
        return Err(ParseError::MissingOpeningDelimiter);
    };

    let mut search_from = 0;
    while let Some(rel) = rest[search_from..].find("\n---") {
        let pos = search_from + rel;
        let after = pos + "\n---".len();
        let tail = &rest[after..];
        if tail.is_empty() {
            return Ok((&rest[..pos], ""));
        }
        if let Some(body) = tail.strip_prefix('\n') {
            return Ok((&rest[..pos], body));
        }
        if let Some(body) = tail.strip_prefix("\r\n") {
            return Ok((&rest[..pos], body));
        }
        // `\n---` followed by other text (e.g. `\n---ignored`) is not
        // a delimiter line; advance and keep searching.
        search_from = after;
    }
    Err(ParseError::MissingClosingDelimiter)
}

/// Recursively rewrite every JSON object key from `snake_case` to
/// kebab-case.
///
/// Codex rule frontmatter keys never contain `_` for any reason
/// other than the `snake_case` authoring convention (`review_mode`,
/// `deterministic_hints`, `replaced_by`), so a blind `_` -> `-`
/// rewrite is safe. String VALUES (e.g. adapter names like
/// `code-typescript`) are untouched — only keys are transformed.
fn snake_to_kebab_keys(value: JsonValue) -> JsonValue {
    match value {
        JsonValue::Object(map) => {
            let mut out = JsonMap::with_capacity(map.len());
            for (k, v) in map {
                out.insert(k.replace('_', "-"), snake_to_kebab_keys(v));
            }
            JsonValue::Object(out)
        }
        JsonValue::Array(items) => {
            JsonValue::Array(items.into_iter().map(snake_to_kebab_keys).collect())
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::{HintKind, ReviewMode, Severity};

    const HARDCODED_CONFIGURATION: &str =
        include_str!("../../tests/fixtures/codex/hardcoded-configuration.md");
    const DOCUMENTATION_VERBATIM_PRESERVATION: &str =
        include_str!("../../tests/fixtures/codex/documentation-verbatim-preservation.md");

    /// Real shared `UNI-014` rule (RFC-28 §Codex file shape worked
    /// example). Frontmatter fields land in the typed shape and the
    /// body carries the policy headings verbatim. The fixture has a
    /// blank line between the closing `---` and `## Rule`, so the
    /// body opens with that preserved blank — only the single
    /// newline immediately after the delimiter is stripped, per
    /// RFC body-fidelity rules.
    #[test]
    fn parses_hardcoded_configuration_fixture() {
        let rule = parse_codex_rule(HARDCODED_CONFIGURATION).expect("parses");
        assert_eq!(rule.id, "UNI-014");
        assert_eq!(rule.title, "Hardcoded Configuration Values");
        assert_eq!(rule.severity, Severity::Important);
        assert!(
            rule.body.starts_with("\n## Rule\n"),
            "body must preserve the blank line before '## Rule', got: {:?}",
            &rule.body[..rule.body.len().min(40)]
        );
        // RFC-28 §Codex file shape: body carries the documented
        // section headings verbatim.
        assert!(rule.body.contains("\n## Look For\n"));
        assert!(rule.body.contains("\n## Spec Guidance\n"));
    }

    /// Real source-axis `SRC-001` rule. Exercises optional
    /// frontmatter blocks (`applicability.adapters`, `references`)
    /// and confirms the body preserves multi-paragraph prose.
    #[test]
    fn parses_documentation_verbatim_preservation_fixture() {
        let rule = parse_codex_rule(DOCUMENTATION_VERBATIM_PRESERVATION).expect("parses");
        assert_eq!(rule.id, "SRC-001");
        assert_eq!(rule.severity, Severity::Important);
        let applicability = rule.applicability.expect("applicability present");
        assert_eq!(applicability.adapters.as_deref(), Some(&["documentation".to_string()][..]));
        let references = rule.references.expect("references present");
        assert_eq!(references.len(), 1);
        assert_eq!(references[0].label, "documentation.extract determinism rules");
        assert_eq!(
            references[0].path.as_deref(),
            Some("adapters/sources/documentation/briefs/extract.md"),
        );
        assert!(rule.body.contains("\n## Rule\n"));
    }

    /// `snake_case` authoring keys lift to the kebab-case wire shape
    /// carried by [`CodexRule`]. Covers every documented rename:
    /// `review_mode`, `deterministic_hints`, and the nested
    /// `deprecated.replaced_by`.
    #[test]
    fn snake_case_keys_lift_to_kebab_case() {
        let content = r"---
id: UNI-014
title: Sample
severity: important
trigger: A short trigger sentence covering the rule context.
review_mode: hybrid
deterministic_hints:
  - kind: regex
    value: 'https?://'
    description: Literal URL.
deprecated:
  reason: superseded by SEC-001
  replaced_by: SEC-001
---
## Rule

Body.
";
        let rule = parse_codex_rule(content).expect("parses");
        assert_eq!(rule.review_mode, Some(ReviewMode::Hybrid));
        let hints = rule.deterministic_hints.as_ref().expect("hints present");
        assert_eq!(hints.len(), 1);
        assert_eq!(hints[0].kind, HintKind::Regex);
        assert_eq!(hints[0].value, "https?://");
        let deprecated = rule.deprecated.as_ref().expect("deprecated present");
        assert_eq!(deprecated.reason, "superseded by SEC-001");
        assert_eq!(deprecated.replaced_by.as_deref(), Some("SEC-001"));

        // Re-serialize and confirm the kebab-case wire form is
        // intact (no snake_case key leaks).
        let json = serde_json::to_string(&rule).expect("serialize");
        assert!(json.contains("\"review-mode\""));
        assert!(json.contains("\"deterministic-hints\""));
        assert!(json.contains("\"replaced-by\""));
        assert!(!json.contains("\"review_mode\""));
        assert!(!json.contains("\"deterministic_hints\""));
        assert!(!json.contains("\"replaced_by\""));
    }

    /// Body bytes are preserved verbatim, including code fences
    /// containing `---` separators and inner blank lines. RFC-28
    /// §Codex file shape: reviewers consume the body as policy
    /// text, so any byte-level edit here is a correctness break.
    #[test]
    fn body_is_preserved_verbatim_with_code_fences() {
        let content = "---\n\
id: UNI-014\n\
title: Body fidelity\n\
severity: optional\n\
trigger: Body preservation regression covering fenced ``` blocks with inner separators.\n\
---\n\
## Rule\n\
\n\
Some prose.\n\
\n\
```yaml\n\
key: value\n\
---\n\
other: doc\n\
```\n\
\n\
Trailing line.\n";
        let rule = parse_codex_rule(content).expect("parses");
        let expected_body = "## Rule\n\
\n\
Some prose.\n\
\n\
```yaml\n\
key: value\n\
---\n\
other: doc\n\
```\n\
\n\
Trailing line.\n";
        assert_eq!(rule.body, expected_body);
    }

    /// CRLF line endings on the closing delimiter and inside the
    /// body round-trip into the typed rule. The leading newline
    /// after the closing `---\r\n` is stripped, every other byte
    /// is preserved.
    #[test]
    fn body_preserves_crlf_line_endings() {
        let content = "---\r\nid: UNI-014\r\ntitle: CRLF\r\nseverity: optional\r\ntrigger: CRLF body fidelity covering Windows-style line endings end-to-end.\r\n---\r\n## Rule\r\n\r\nLine one.\r\nLine two.\r\n";
        let rule = parse_codex_rule(content).expect("parses");
        assert_eq!(rule.body, "## Rule\r\n\r\nLine one.\r\nLine two.\r\n");
    }

    /// Missing leading `---` line surfaces as
    /// [`ParseError::MissingOpeningDelimiter`].
    #[test]
    fn missing_opening_delimiter_errors() {
        let content = "## Rule\nNo frontmatter at all.\n";
        let err = parse_codex_rule(content).expect_err("must error");
        assert!(matches!(err, ParseError::MissingOpeningDelimiter), "got: {err:?}");
    }

    /// Opening delimiter without a matching close surfaces as
    /// [`ParseError::MissingClosingDelimiter`].
    #[test]
    fn missing_closing_delimiter_errors() {
        let content = "---\nid: UNI-014\ntitle: dangling\nseverity: optional\ntrigger: t.\n";
        let err = parse_codex_rule(content).expect_err("must error");
        assert!(matches!(err, ParseError::MissingClosingDelimiter), "got: {err:?}");
    }

    /// Unparseable YAML in the frontmatter surfaces as
    /// [`ParseError::Yaml`].
    #[test]
    fn invalid_yaml_errors() {
        let content = "---\nid: UNI-014\n  bad: : indent\n---\n## Rule\n";
        let err = parse_codex_rule(content).expect_err("must error");
        assert!(matches!(err, ParseError::Yaml(_)), "got: {err:?}");
    }

    /// Schema-mandated `id` field missing surfaces as
    /// [`ParseError::Schema`].
    #[test]
    fn schema_violation_errors_when_id_missing() {
        let content = "---\ntitle: No Id\nseverity: important\ntrigger: t.\n---\n## Rule\n";
        let err = parse_codex_rule(content).expect_err("must error");
        assert!(matches!(err, ParseError::Schema(_)), "got: {err:?}");
        if let ParseError::Schema(detail) = err {
            assert!(detail.contains("id"), "expected 'id' in schema detail, got: {detail}");
        }
    }

    /// RFC-28 §Deterministic hints extensibility: "the runtime
    /// resolver MUST NOT compile a regex it never executes". A hint
    /// with an unparseable regex pattern MUST still parse — regex
    /// compilation is RFC-32 / CH-13 territory.
    #[test]
    fn invalid_regex_hint_value_still_parses() {
        let content = r"---
id: UNI-014
title: Broken regex tolerated
severity: optional
trigger: Parser must not compile regex hint values; that belongs to hint execution.
deterministic_hints:
  - kind: regex
    value: '[invalid regex)('
---
## Rule
";
        let rule = parse_codex_rule(content).expect("parses despite broken regex value");
        let hints = rule.deterministic_hints.expect("hints present");
        assert_eq!(hints.len(), 1);
        assert_eq!(hints[0].kind, HintKind::Regex);
        assert_eq!(hints[0].value, "[invalid regex)(");
    }

    /// RFC-32 reserved hint kinds (`set-coverage`, etc.) round-trip
    /// successfully even though no execution semantics exist for
    /// them yet.
    #[test]
    fn reserved_hint_kinds_round_trip() {
        let content = r"---
id: UNI-014
title: Reserved hint kinds
severity: optional
trigger: RFC-32 reserved hint kinds must shape-validate without execution semantics.
deterministic_hints:
  - kind: set-coverage
    value: 'rule.id'
  - kind: namespace-owner
    value: 'UNI'
---
## Rule
";
        let rule = parse_codex_rule(content).expect("parses");
        let hints = rule.deterministic_hints.expect("hints present");
        assert_eq!(hints.len(), 2);
        assert_eq!(hints[0].kind, HintKind::SetCoverage);
        assert_eq!(hints[1].kind, HintKind::NamespaceOwner);
    }

    /// Helper: snake-to-kebab lift only rewrites keys, never
    /// values. Adapter names like `code-typescript` must survive
    /// untouched.
    #[test]
    fn snake_to_kebab_only_touches_keys() {
        let input = serde_json::json!({
            "review_mode": "hybrid",
            "applicability": {
                "adapters": ["code-typescript", "documentation"],
            },
            "deterministic_hints": [
                {"kind": "regex", "value": "snake_in_value_stays"},
            ],
        });
        let lifted = snake_to_kebab_keys(input);
        assert_eq!(lifted["review-mode"], "hybrid");
        assert_eq!(lifted["applicability"]["adapters"][0], "code-typescript");
        assert_eq!(lifted["deterministic-hints"][0]["value"], "snake_in_value_stays");
    }
}
