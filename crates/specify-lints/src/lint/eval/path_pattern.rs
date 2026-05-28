//! `kind: path-pattern` evaluator per the executable hint-kind contract.
//!
//! Path-pattern hints are filters, not finders. They narrow the
//! candidate file set the later hint kinds (`schema`, `regex`,
//! `tool`) consume; they emit zero findings on their own. Glob
//! semantics follow the [`glob::Pattern`] crate (already a workspace
//! dependency for the codex resolver). The pattern matches a file's
//! project-relative path verbatim — no separator translation, no
//! per-OS munging, since [`crate::lint::File::path`] is already
//! forward-slash relative per `WorkspaceModel` stability.
//!
//! Negation prefixes (`!pattern`) are reserved: v1 has no defined
//! semantics for "exclude these files from the candidate set" and
//! the runner refuses them with [`super::HintError::Unsupported`].

use std::path::PathBuf;

use glob::Pattern;

use super::HintError;
use crate::lint::WorkspaceModel;
use crate::rules::{DeterministicHint, HintKind, ResolvedRule};

pub(crate) fn evaluate(
    rule: &ResolvedRule, hint: &DeterministicHint, model: &WorkspaceModel,
) -> Result<Vec<PathBuf>, HintError> {
    if hint.value.starts_with('!') {
        return Err(HintError::Unsupported {
            rule_id: rule.rule_id.clone(),
            kind: HintKind::PathPattern,
            reason: "negated path-pattern globs are reserved",
        });
    }
    let pattern = Pattern::new(&hint.value).map_err(|_silenced| HintError::Unsupported {
        rule_id: rule.rule_id.clone(),
        kind: HintKind::PathPattern,
        reason: "invalid glob pattern",
    })?;
    let mut matches: Vec<PathBuf> = model
        .files
        .iter()
        .filter(|f| pattern.matches(&f.path))
        .map(|f| PathBuf::from(&f.path))
        .collect();
    matches.sort();
    Ok(matches)
}
