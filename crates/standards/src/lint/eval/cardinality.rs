//! `kind: cardinality` evaluator.
//!
//! Asserts that some countable property of a candidate file is within
//! configured bounds. v1 supports one source discriminator —
//! `skill-body-line-count-max-200` — which consumes the
//! [`crate::lint::Skill`] facts the framework-profile indexer already
//! produced (see [`crate::lint::index::skill::extract`], whose
//! `body_line_count` field is the canonical count of non-frontmatter
//! body lines) and flags each `plugins/<plugin>/skills/<skill>/SKILL.md`
//! whose body exceeds the 200-line cap pinned by
//! [`docs/standards/skill-authoring.md`](https://github.com/augentic/specify/blob/main/docs/standards/skill-authoring.md).
//! The interpreter emits one [`specify_diagnostics::Diagnostic`] per
//! over-budget skill with the SKILL.md path as the finding's location
//! and the `(actual, max)` pair surfaced via
//! [`specify_diagnostics::FindingEvidence::Structured`] for downstream
//! tooling.
//!
//! Skills whose `path` is not in the caller-supplied candidate set
//! are ignored, so the closed `path-pattern` filter the umbrella
//! evaluator builds still drives candidate selection. Skills the
//! indexer drops upstream (missing frontmatter, malformed path,
//! missing `name:`) never reach this layer; skills whose
//! `body_line_count` is `None` (consumer-profile facts) are skipped
//! silently.
//!
//! The single source discriminator hardcodes both the metric
//! (`Skill.body_line_count`) and the upper bound (200) — the
//! predicate-migration map pins exactly this cap, so a richer config
//! shape (`metric: …, max: …`) is deferred until a second consumer
//! arrives. Future hint values may extend the closed source set;
//! unknown discriminators are rejected as
//! [`super::HintError::Unsupported`] so authoring drift surfaces at
//! hint-evaluation time rather than silently passing.

use std::path::PathBuf;

use specify_diagnostics::{Diagnostic, FindingEvidence, FindingLocation};

use super::{HintError, make_finding};
use crate::lint::WorkspaceModel;
use crate::rules::{DeterministicHint, HintKind, ResolvedRule};

const SOURCE_SKILL_BODY_LINE_COUNT_MAX_200: &str = "skill-body-line-count-max-200";
const SKILL_BODY_LINE_MAX: u32 = 200;

pub(crate) fn evaluate(
    rule: &ResolvedRule, hint: &DeterministicHint, candidates: &[PathBuf], model: &WorkspaceModel,
    next_id: &mut u64,
) -> Result<Vec<Diagnostic>, HintError> {
    let source = hint.value.trim();
    if source != SOURCE_SKILL_BODY_LINE_COUNT_MAX_200 {
        return Err(HintError::Unsupported {
            rule_id: rule.rule_id.clone(),
            kind: HintKind::Cardinality,
            reason: "only `skill-body-line-count-max-200` is supported in v1",
        });
    }

    let candidate_set = super::candidate_set(candidates);

    let mut out: Vec<Diagnostic> = Vec::new();
    for skill in &model.skills {
        if !candidate_set.contains(&skill.path) {
            continue;
        }
        let Some(actual) = skill.body_line_count else { continue };
        if actual <= SKILL_BODY_LINE_MAX {
            continue;
        }
        let location = FindingLocation {
            path: skill.path.clone(),
            line: Some(1),
            column: None,
            end_line: None,
            end_column: None,
        };
        let evidence = FindingEvidence::Structured {
            summary: format!(
                "skill '{}' body has {} lines (limit {})",
                skill.name, actual, SKILL_BODY_LINE_MAX,
            ),
            data: serde_json::json!({
                "skill": skill.name,
                "path": skill.path,
                "actual": actual,
                "max": SKILL_BODY_LINE_MAX,
            }),
            locations: None,
        };
        let title = format!(
            "{}: skill '{}' body exceeds the {}-line cap ({} lines)",
            rule.title, skill.name, SKILL_BODY_LINE_MAX, actual,
        );
        let finding = make_finding(rule, *next_id, title, Some(location), evidence);
        *next_id += 1;
        out.push(finding);
    }
    Ok(out)
}
