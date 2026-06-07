//! `kind: unique` evaluator.
//!
//! Asserts that some field across a set of candidate files is unique.
//! v1 supports one fact-family selector — `skill` — over the
//! [`crate::lint::Skill`] facts the framework-profile indexer already
//! produced (see [`crate::lint::index::skill`]). The field to enforce
//! uniqueness on is **the field selector the rule supplies in
//! `config: { field }`** (`skill-name`), not a `const` discriminator in
//! this arm; v1 understands `skill-name`, flagging each `name:` value
//! that appears on two or more `plugins/**/SKILL.md` files. The
//! interpreter emits one
//! [`specify_diagnostics::Diagnostic`] per duplicated name, with the lowest
//! offending path used as the finding's location and the full sorted
//! path list surfaced via [`specify_diagnostics::FindingEvidence::Structured`].
//!
//! Skills whose `path` is not in the caller-supplied candidate set
//! are ignored, so the closed `path-pattern` filter the umbrella
//! evaluator builds still drives candidate selection. Skills missing
//! a frontmatter `name:` value are dropped upstream by
//! [`crate::lint::index::skill::extract`] and never reach this layer.
//!
//! Future hint values may extend the closed selector / field sets;
//! unknown selectors and fields are rejected as
//! [`super::HintError::Unsupported`] so authoring drift surfaces at
//! hint-evaluation time rather than silently passing.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde::Deserialize;
use specify_diagnostics::{Diagnostic, FindingEvidence, FindingLocation};

use super::{HintError, make_finding};
use crate::lint::WorkspaceModel;
use crate::rules::{HintKind, ResolvedRule, RuleHint};

const SOURCE_SKILL: &str = "skill";
/// The only field this arm can enforce uniqueness on today; naming a
/// fact field is mechanism.
const FIELD_SKILL_NAME: &str = "skill-name";

/// Parsed `unique` hint configuration. The field selector is supplied
/// by the rule.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct UniqueConfig {
    field: String,
}

impl UniqueConfig {
    fn parse(rule: &ResolvedRule, hint: &RuleHint) -> Result<Self, HintError> {
        let raw = hint.config.as_ref().ok_or_else(|| HintError::Unsupported {
            rule_id: rule.rule_id.clone(),
            kind: HintKind::Unique,
            reason: "`skill` requires a `config: { field }`",
        })?;
        serde_json::from_value(raw.clone()).map_err(|_ignored| HintError::Unsupported {
            rule_id: rule.rule_id.clone(),
            kind: HintKind::Unique,
            reason: "invalid unique hint config JSON",
        })
    }
}

pub(crate) fn evaluate(
    rule: &ResolvedRule, hint: &RuleHint, candidates: &[PathBuf], model: &WorkspaceModel,
    next_id: &mut u64,
) -> Result<Vec<Diagnostic>, HintError> {
    let source = hint.value.trim();
    if source != SOURCE_SKILL {
        return Err(HintError::Unsupported {
            rule_id: rule.rule_id.clone(),
            kind: HintKind::Unique,
            reason: "only `skill` is supported in v1",
        });
    }
    let cfg = UniqueConfig::parse(rule, hint)?;
    if cfg.field != FIELD_SKILL_NAME {
        return Err(HintError::Unsupported {
            rule_id: rule.rule_id.clone(),
            kind: HintKind::Unique,
            reason: "only the `skill-name` field is supported in v1",
        });
    }

    let candidate_set = super::candidate_set(candidates);

    let mut groups: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for skill in &model.skills {
        if !candidate_set.contains(&skill.path) {
            continue;
        }
        groups.entry(skill.name.clone()).or_default().insert(skill.path.clone());
    }

    let mut out: Vec<Diagnostic> = Vec::new();
    for (name, paths) in groups {
        if paths.len() < 2 {
            continue;
        }
        let sorted: Vec<String> = paths.into_iter().collect();
        debug_assert!(sorted.len() >= 2, "duplicate group filtered to < 2 paths");
        let Some(first) = sorted.first().cloned() else {
            continue;
        };
        let location = FindingLocation {
            path: first,
            line: Some(1),
            column: None,
            end_line: None,
            end_column: None,
        };
        let locations: Vec<FindingLocation> = sorted
            .iter()
            .map(|p| FindingLocation {
                path: p.clone(),
                line: Some(1),
                column: None,
                end_line: None,
                end_column: None,
            })
            .collect();
        let evidence = FindingEvidence::Structured {
            summary: format!("duplicate skill name '{name}'"),
            data: serde_json::json!({ "name": name, "paths": sorted }),
            locations: Some(locations),
        };
        let title = format!("{}: duplicate skill name '{}'", rule.title, name);
        let finding = make_finding(rule, *next_id, title, Some(location), evidence);
        *next_id += 1;
        out.push(finding);
    }
    Ok(out)
}
