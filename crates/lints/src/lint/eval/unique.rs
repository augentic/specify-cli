//! `kind: unique` evaluator per RFC-34 §F6 PR 3.
//!
//! Asserts that some field across a set of candidate files is unique.
//! v1 supports one source discriminator — `skill-name` — which
//! consumes the [`crate::lint::Skill`] facts the framework-profile
//! indexer already produced (see [`crate::lint::index::skill`]) and
//! flags each `name:` frontmatter value that appears on two or more
//! `plugins/**/SKILL.md` files. The interpreter emits one
//! [`crate::rules::LintFinding`] per duplicated name, with the lowest
//! offending path used as the finding's location and the full sorted
//! path list surfaced via [`crate::rules::FindingEvidence::Structured`].
//!
//! Skills whose `path` is not in the caller-supplied candidate set
//! are ignored, so the closed `path-pattern` filter the umbrella
//! evaluator builds still drives candidate selection. Skills missing
//! a frontmatter `name:` value are dropped upstream by
//! [`crate::lint::index::skill::extract`] and never reach this layer.
//!
//! Future hint values may extend the closed source set (e.g.
//! `adapter-name`, `rule-id`); unknown discriminators are rejected as
//! [`super::HintError::Unsupported`] so authoring drift surfaces at
//! hint-evaluation time rather than silently passing.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use super::{HintError, make_finding};
use crate::lint::WorkspaceModel;
use crate::rules::{
    DeterministicHint, FindingEvidence, FindingLocation, HintKind, LintFinding, ResolvedRule,
};

const SOURCE_SKILL_NAME: &str = "skill-name";

pub(crate) fn evaluate(
    rule: &ResolvedRule, hint: &DeterministicHint, candidates: &[PathBuf], model: &WorkspaceModel,
    next_id: &mut u64,
) -> Result<Vec<LintFinding>, HintError> {
    let source = hint.value.trim();
    if source != SOURCE_SKILL_NAME {
        return Err(HintError::Unsupported {
            rule_id: rule.rule_id.clone(),
            kind: HintKind::Unique,
            reason: "only `skill-name` is supported in v1",
        });
    }

    let candidate_set: BTreeSet<String> =
        candidates.iter().map(|p| p.to_string_lossy().into_owned()).collect();

    let mut groups: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for skill in &model.skills {
        if !candidate_set.contains(&skill.path) {
            continue;
        }
        groups.entry(skill.name.clone()).or_default().insert(skill.path.clone());
    }

    let mut out: Vec<LintFinding> = Vec::new();
    for (name, paths) in groups {
        if paths.len() < 2 {
            continue;
        }
        let sorted: Vec<String> = paths.into_iter().collect();
        let first = sorted.first().cloned().expect("len >= 2");
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
