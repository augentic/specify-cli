//! `kind: reference-resolves` evaluator.
//!
//! Asserts that every reference of the requested kind that originates
//! from a candidate file resolves to a real path on disk. v1 supports
//! one source discriminator — `markdown-link` — which consumes the
//! [`crate::lint::MarkdownLink`] facts the indexer already produced
//! and whose `resolves` flag the umbrella sequential pass populates by
//! joining each link target against `from_path`'s parent and looking
//! it up in the discovered file set (see [`crate::lint::index::build`]
//! `resolve_link`). The interpreter emits one [`specify_diagnostics::Diagnostic`]
//! per `resolves == Some(false)` link, with a 1-indexed `line`
//! location and the raw target captured in [`specify_diagnostics::FindingEvidence::Snippet`].
//!
//! URL-style targets (`https://…`, `mailto://…`, anchor-only `#frag`,
//! etc.) leave `resolves` unset upstream and are silently skipped here
//! — the interpreter only fires on references the resolver attempted
//! and rejected.
//!
//! Future hint values may extend the closed source set
//! (`yaml-anchor`, `regex:<pattern>`, …); unknown discriminators are
//! rejected as [`super::HintError::Unsupported`] so authoring drift
//! surfaces at hint-evaluation time rather than silently passing.

use std::path::PathBuf;

use specify_diagnostics::{Diagnostic, FindingEvidence, FindingLocation};

use super::{HintError, make_finding};
use crate::lint::WorkspaceModel;
use crate::rules::{HintKind, ResolvedRule, RuleHint};

const SOURCE_MARKDOWN_LINK: &str = "markdown-link";

pub(crate) fn evaluate(
    rule: &ResolvedRule, hint: &RuleHint, candidates: &[PathBuf], model: &WorkspaceModel,
    next_id: &mut u64,
) -> Result<Vec<Diagnostic>, HintError> {
    let source = hint.value.trim();
    if source != SOURCE_MARKDOWN_LINK {
        return Err(HintError::Unsupported {
            rule_id: rule.rule_id.clone(),
            kind: HintKind::ReferenceResolves,
            reason: "only `markdown-link` is supported in v1",
        });
    }

    let candidate_set = super::candidate_set(candidates);

    let mut out: Vec<Diagnostic> = Vec::new();
    for link in &model.markdown_links {
        if !candidate_set.contains(&link.from_path) {
            continue;
        }
        if link.resolves != Some(false) {
            continue;
        }
        let location = FindingLocation {
            path: link.from_path.clone(),
            line: Some(link.line),
            column: None,
            end_line: None,
            end_column: None,
        };
        let evidence = FindingEvidence::Snippet {
            value: link.to_raw.clone(),
        };
        let title = format!("{}: unresolved reference `{}`", rule.title, link.to_raw);
        let finding = make_finding(rule, *next_id, title, Some(location), evidence);
        *next_id += 1;
        out.push(finding);
    }
    Ok(out)
}
