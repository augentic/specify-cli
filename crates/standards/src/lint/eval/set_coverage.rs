//! `kind: set-coverage` evaluator.
//!
//! Asserts that the set of values some candidate file declares
//! covers a closed expected set.
//!
//! `set-coverage` has two complementary directions, selected by the
//! `value` source discriminator:
//!
//! - `adapter-briefs` — the **expected ⊆ declared** direction: an
//!   adapter's `briefs.keys()` must cover the axis-appropriate operation
//!   set the rule supplies in `config: { expected-operations }`; missing
//!   operations are flagged. Extras are silent (`kind: set-eq` tightens
//!   that). The expected sets are **policy supplied by the rule file**,
//!   never a `const` in this arm (per the standards-layer
//!   policy-in-`specify` rule). It consumes the
//!   [`crate::lint::AdapterManifest`] facts the framework-profile
//!   indexer already produced (see [`crate::lint::index::adapter::extract`])
//!   and emits one [`specify_diagnostics::Diagnostic`] per
//!   `(adapter, missing-operation)` pair, with the manifest path as the
//!   finding's location and the per-adapter `(missing, expected, actual)`
//!   triple surfaced via [`specify_diagnostics::FindingEvidence::Structured`].
//! - `skill-allowed-tools` — the **declared ⊆ allowed** direction: every
//!   tool a skill lists in its `allowed-tools` frontmatter must be
//!   covered by the rule's `config: { allowed }` set (optionally with
//!   `allowed-prefixes` exemptions, e.g. `mcp__`); tools not covered are
//!   flagged. The recognised-tool set and prefix exemptions are
//!   **policy supplied by the rule file**, never a `const` in this arm
//!   (per the standards-layer policy-in-`specify` rule).
//!
//! Facts whose `path` is not in the caller-supplied candidate set are
//! ignored, so the closed `path-pattern` filter the umbrella evaluator
//! builds still drives candidate selection. Unknown discriminators are
//! rejected as [`super::HintError::Unsupported`] so authoring drift
//! surfaces at hint-evaluation time rather than silently passing.

use std::collections::BTreeSet;
use std::path::PathBuf;

use serde::Deserialize;
use serde_json::Value as JsonValue;
use specify_diagnostics::{Diagnostic, FindingEvidence, FindingLocation};

use super::{HintError, make_finding};
use crate::lint::WorkspaceModel;
use crate::lint::adapter_briefs::{ExpectedOperationsConfig, axis_token};
use crate::rules::{HintKind, ResolvedRule, RuleHint};

const SOURCE_ADAPTER_BRIEFS: &str = "adapter-briefs";
const SOURCE_SKILL_ALLOWED_TOOLS: &str = "skill-allowed-tools";

/// Parsed `skill-allowed-tools` hint configuration. Both the recognised
/// tool set and the prefix exemptions are policy supplied by the rule.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct AllowedToolsConfig {
    /// The closed set of recognised tool names.
    allowed: Vec<String>,
    /// Prefixes that exempt a tool from the allow-list (e.g. `mcp__`
    /// for dynamically-named MCP tools).
    #[serde(default)]
    allowed_prefixes: Vec<String>,
}

impl AllowedToolsConfig {
    fn parse(rule: &ResolvedRule, hint: &RuleHint) -> Result<Self, HintError> {
        let raw = hint.config.as_ref().ok_or_else(|| HintError::Unsupported {
            rule_id: rule.rule_id.clone(),
            kind: HintKind::SetCoverage,
            reason: "`skill-allowed-tools` requires a `config: { allowed }`",
        })?;
        serde_json::from_value(raw.clone()).map_err(|_ignored| HintError::Unsupported {
            rule_id: rule.rule_id.clone(),
            kind: HintKind::SetCoverage,
            reason: "invalid set-coverage hint config JSON",
        })
    }

    fn covers(&self, tool: &str) -> bool {
        self.allowed.iter().any(|allowed| allowed == tool)
            || self.allowed_prefixes.iter().any(|prefix| tool.starts_with(prefix.as_str()))
    }
}

pub(crate) fn evaluate(
    rule: &ResolvedRule, hint: &RuleHint, candidates: &[PathBuf], model: &WorkspaceModel,
    next_id: &mut u64,
) -> Result<Vec<Diagnostic>, HintError> {
    match hint.value.trim() {
        SOURCE_ADAPTER_BRIEFS => {
            let cfg = ExpectedOperationsConfig::parse(hint.config.as_ref()).ok_or_else(|| {
                HintError::Unsupported {
                    rule_id: rule.rule_id.clone(),
                    kind: HintKind::SetCoverage,
                    reason: "`adapter-briefs` requires a `config: { expected-operations }`",
                }
            })?;
            Ok(adapter_briefs(rule, candidates, model, &cfg, next_id))
        }
        SOURCE_SKILL_ALLOWED_TOOLS => {
            let cfg = AllowedToolsConfig::parse(rule, hint)?;
            Ok(skill_allowed_tools(rule, candidates, model, &cfg, next_id))
        }
        _ => Err(HintError::Unsupported {
            rule_id: rule.rule_id.clone(),
            kind: HintKind::SetCoverage,
            reason: "unknown set-coverage source discriminator",
        }),
    }
}

fn adapter_briefs(
    rule: &ResolvedRule, candidates: &[PathBuf], model: &WorkspaceModel,
    cfg: &ExpectedOperationsConfig, next_id: &mut u64,
) -> Vec<Diagnostic> {
    let candidate_set = super::candidate_set(candidates);

    let mut out: Vec<Diagnostic> = Vec::new();
    for manifest in &model.adapter_manifests {
        if !candidate_set.contains(&manifest.path) {
            continue;
        }
        let expected = cfg.expected_for(manifest.axis);
        let actual: BTreeSet<&str> = manifest.brief_keys.iter().map(String::as_str).collect();
        let mut missing: Vec<&str> =
            expected.iter().copied().filter(|op| !actual.contains(op)).collect();
        if missing.is_empty() {
            continue;
        }
        missing.sort_unstable();
        let expected_sorted: Vec<String> = expected.iter().map(|s| (*s).to_string()).collect();
        let actual_sorted: Vec<String> = actual.iter().map(|s| (*s).to_string()).collect();
        for op in missing {
            let location = FindingLocation {
                path: manifest.path.clone(),
                line: Some(1),
                column: None,
                end_line: None,
                end_column: None,
            };
            let evidence = FindingEvidence::Structured {
                summary: format!(
                    "adapter '{}' is missing brief for operation '{}'",
                    manifest.name, op,
                ),
                data: serde_json::json!({
                    "adapter": manifest.name,
                    "axis": axis_token(manifest.axis),
                    "missing": op,
                    "expected": expected_sorted,
                    "actual": actual_sorted,
                }),
                locations: None,
            };
            let title = format!(
                "{}: adapter '{}' missing brief for operation '{}'",
                rule.title, manifest.name, op,
            );
            let finding = make_finding(rule, *next_id, title, Some(location), evidence);
            *next_id += 1;
            out.push(finding);
        }
    }
    out
}

/// Flag every `allowed-tools` entry on a candidate skill that the rule's
/// `allowed` set (plus `allowed-prefixes` exemptions) does not cover.
/// One finding per uncovered `(skill, tool)` pair.
fn skill_allowed_tools(
    rule: &ResolvedRule, candidates: &[PathBuf], model: &WorkspaceModel, cfg: &AllowedToolsConfig,
    next_id: &mut u64,
) -> Vec<Diagnostic> {
    let candidate_set = super::candidate_set(candidates);

    let mut out: Vec<Diagnostic> = Vec::new();
    for frontmatter in &model.frontmatter {
        if !candidate_set.contains(&frontmatter.path) {
            continue;
        }
        let Some(tools) = frontmatter.fields.get("allowed-tools").and_then(JsonValue::as_str)
        else {
            continue;
        };
        for tool in tools.split_whitespace().filter(|t| !t.is_empty()) {
            if cfg.covers(tool) {
                continue;
            }
            let location = FindingLocation {
                path: frontmatter.path.clone(),
                line: Some(1),
                column: None,
                end_line: None,
                end_column: None,
            };
            let evidence = FindingEvidence::Structured {
                summary: format!("unrecognised tool '{tool}' in allowed-tools"),
                data: serde_json::json!({
                    "path": frontmatter.path,
                    "tool": tool,
                }),
                locations: None,
            };
            let title = format!("{}: unrecognised tool '{}' in allowed-tools", rule.title, tool);
            let finding = make_finding(rule, *next_id, title, Some(location), evidence);
            *next_id += 1;
            out.push(finding);
        }
    }
    out
}
