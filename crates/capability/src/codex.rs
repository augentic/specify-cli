//! Codex rule frontmatter + body parsing.
//!
//! Codex rules are markdown files with a leading YAML frontmatter block
//! followed by a markdown body. This module validates one rule file at a
//! time; project-aware resolution, provenance, and duplicate-id checks
//! are layered above the single-file parser.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use specify_error::{Error, ValidationStatus, ValidationSummary};

use crate::ValidationResult;
use crate::brief::split_on_closing_delimiter;
use crate::capability::validate_against_schema;

const CODEX_RULE_JSON_SCHEMA: &str = include_str!("../../../schemas/codex-rule.schema.json");

const RULE_FRONTMATTER_DELIMITED: &str = "codex.frontmatter-delimited";
const RULE_FRONTMATTER_PARSEABLE: &str = "codex.frontmatter-parseable";
const RULE_FRONTMATTER_VALID: &str = "codex.frontmatter-valid";
const RULE_BODY_HAS_RULE_HEADING: &str = "codex.body-has-rule-heading";

/// Parsed codex rule file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CodexRule {
    /// Filesystem path the rule was loaded from.
    pub path: PathBuf,
    /// Parsed YAML frontmatter.
    pub frontmatter: CodexRuleFrontmatter,
    /// Markdown body after the closing `---` delimiter.
    pub body: String,
    /// Canonical rule id used for lookups and duplicate-id checks.
    pub normalized_id: String,
}

/// Parsed frontmatter of a codex rule markdown file.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CodexRuleFrontmatter {
    /// Stable rule identifier, e.g. `UNI-002`.
    pub id: String,
    /// Short human-readable rule title.
    pub title: String,
    /// Default review severity.
    pub severity: CodexSeverity,
    /// One-sentence condition that tells reviewers when the rule matters.
    pub trigger: String,
    /// Optional applicability metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applicability: Option<CodexApplicability>,
    /// Optional review execution classification.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_mode: Option<CodexReviewMode>,
    /// Optional deterministic-review hints.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deterministic_hints: Vec<CodexDeterministicHint>,
    /// Optional reference material.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub references: Vec<CodexReference>,
    /// Optional deprecation metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deprecated: Option<CodexDeprecation>,
}

/// Canonical codex finding severity.
#[derive(Debug, Copy, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum CodexSeverity {
    /// Must-fix issue that can cause serious correctness, security, or data loss.
    Critical,
    /// Important issue that should normally block landing.
    Important,
    /// Improvement suggestion that should be considered during review.
    Suggestion,
    /// Optional guidance that may be useful but is not expected to block.
    Optional,
}

/// How a codex rule is expected to be reviewed.
#[derive(Debug, Copy, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum CodexReviewMode {
    /// Fully deterministic rule.
    Deterministic,
    /// Rule that requires model judgment.
    ModelAssisted,
    /// Rule that combines deterministic signals with model judgment.
    Hybrid,
}

/// Optional V1 applicability filters.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CodexApplicability {
    /// Capability identifiers, optionally versioned as `<name>@v<major>`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,
    /// Lowercase language or format tokens.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub languages: Vec<String>,
    /// Lowercase artifact category tokens.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<String>,
    /// Relative path or glob patterns.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
}

/// Optional deterministic-review hint.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CodexDeterministicHint {
    /// Kind of deterministic signal the hint describes.
    pub kind: CodexHintKind,
    /// Hint payload, interpreted by a future validator or review tool.
    pub value: String,
    /// Optional human explanation for the hint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Supported deterministic hint kinds.
#[derive(Debug, Copy, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum CodexHintKind {
    /// Filesystem path pattern.
    PathPattern,
    /// Regular expression.
    Regex,
    /// Schema reference.
    Schema,
    /// Tool name or selector.
    Tool,
}

/// Optional codex rule reference.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CodexReference {
    /// Short display label for the reference.
    pub label: String,
    /// HTTP(S) reference URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Repository-relative reference path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// Optional codex rule deprecation metadata.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CodexDeprecation {
    /// Human-readable reason the rule is deprecated.
    pub reason: String,
    /// Replacement codex rule id, when there is a direct successor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replaced_by: Option<String>,
}

impl CodexRule {
    /// Read `path` and parse it via [`CodexRule::parse`].
    ///
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    pub fn load(path: &Path) -> Result<Self, Error> {
        let contents = std::fs::read_to_string(path).map_err(|err| Error::Diag {
            code: "codex-rule-read-failed",
            detail: format!("failed to read {}: {err}", path.display()),
        })?;
        Self::parse(path, &contents)
    }

    /// Parse an in-memory codex rule after running deterministic format
    /// validation.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Validation`] when the rule file does not satisfy
    /// the codex rule format, or an `Error::Diag` when a post-validation
    /// parser invariant fails.
    pub fn parse(path: &Path, contents: &str) -> Result<Self, Error> {
        let results = Self::validate_str(path, contents);
        let failures = validation_failures(&results);
        if !failures.is_empty() {
            return Err(Error::Validation {
                count: failures.len(),
                results: failures,
            });
        }

        let (frontmatter_text, body) = frontmatter_parts(path, contents)?;
        let frontmatter: CodexRuleFrontmatter =
            serde_saphyr::from_str(frontmatter_text).map_err(|err| Error::Diag {
                code: "codex-rule-frontmatter-unreadable",
                detail: format!(
                    "{} frontmatter passed validation but could not be parsed: {err}",
                    path.display()
                ),
            })?;
        let normalized_id = frontmatter.id.to_ascii_uppercase();
        Ok(Self {
            path: path.to_path_buf(),
            frontmatter,
            body: body.to_string(),
            normalized_id,
        })
    }

    /// Validate an in-memory codex rule file without constructing a
    /// [`CodexRule`].
    #[must_use]
    pub fn validate_str(path: &Path, contents: &str) -> Vec<ValidationResult> {
        let (frontmatter_text, body) = match frontmatter_parts(path, contents) {
            Ok(parts) => parts,
            Err(err) => {
                return vec![ValidationResult::Fail {
                    rule_id: RULE_FRONTMATTER_DELIMITED,
                    rule: "codex rule has leading YAML frontmatter delimiters",
                    detail: err.to_string(),
                }];
            }
        };

        let mut results = vec![ValidationResult::Pass {
            rule_id: RULE_FRONTMATTER_DELIMITED,
            rule: "codex rule has leading YAML frontmatter delimiters",
        }];

        let frontmatter_value: serde_json::Value = match serde_saphyr::from_str(frontmatter_text) {
            Ok(value) => value,
            Err(err) => {
                results.push(ValidationResult::Fail {
                    rule_id: RULE_FRONTMATTER_PARSEABLE,
                    rule: "codex rule frontmatter parses as YAML",
                    detail: format!(
                        "codex-rule-frontmatter-malformed: {} has invalid frontmatter YAML: {err}",
                        path.display()
                    ),
                });
                return results;
            }
        };

        results.push(ValidationResult::Pass {
            rule_id: RULE_FRONTMATTER_PARSEABLE,
            rule: "codex rule frontmatter parses as YAML",
        });
        results.extend(validate_against_schema(
            CODEX_RULE_JSON_SCHEMA,
            RULE_FRONTMATTER_VALID,
            "codex rule frontmatter conforms to schemas/codex-rule.schema.json",
            &frontmatter_value,
        ));
        results.push(validate_rule_heading(path, body));
        results
    }
}

fn frontmatter_parts<'a>(path: &Path, contents: &'a str) -> Result<(&'a str, &'a str), Error> {
    let stripped = contents
        .strip_prefix("---\n")
        .or_else(|| contents.strip_prefix("---\r\n"))
        .ok_or_else(|| Error::Diag {
            code: "codex-rule-frontmatter-missing",
            detail: format!("{} is missing a leading `---` frontmatter delimiter", path.display()),
        })?;

    split_on_closing_delimiter(stripped).ok_or_else(|| Error::Diag {
        code: "codex-rule-frontmatter-unclosed",
        detail: format!("{} has an opening `---` but no closing `---` delimiter", path.display()),
    })
}

fn validate_rule_heading(path: &Path, body: &str) -> ValidationResult {
    if has_rule_heading(body) {
        ValidationResult::Pass {
            rule_id: RULE_BODY_HAS_RULE_HEADING,
            rule: "codex rule body contains a `## Rule` heading",
        }
    } else {
        ValidationResult::Fail {
            rule_id: RULE_BODY_HAS_RULE_HEADING,
            rule: "codex rule body contains a `## Rule` heading",
            detail: format!(
                "codex-rule-heading-missing: {} body must contain a `## Rule` heading",
                path.display()
            ),
        }
    }
}

fn has_rule_heading(body: &str) -> bool {
    body.lines().any(|line| line.trim() == "## Rule")
}

fn validation_failures(results: &[ValidationResult]) -> Vec<ValidationSummary> {
    results
        .iter()
        .filter_map(|result| match result {
            ValidationResult::Fail {
                rule_id,
                rule,
                detail,
            } => Some(ValidationSummary {
                status: ValidationStatus::Fail,
                rule_id: (*rule_id).to_string(),
                rule: (*rule).to_string(),
                detail: Some(detail.clone()),
            }),
            _ => None,
        })
        .collect()
}
