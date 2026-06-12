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
//! Include patterns match paths into the candidate set. Exclusion
//! patterns use a leading `!` on `value`; the
//! the eval umbrella unions includes then subtracts excludes.

use std::path::PathBuf;

use glob::Pattern;

use super::HintError;
use crate::lint::WorkspaceModel;
use crate::rules::{HintKind, ResolvedRule, RuleHint};

/// Whether `value` is an exclusion glob (`!` prefix).
#[must_use]
pub(crate) fn is_exclusion(hint: &RuleHint) -> bool {
    hint.value.starts_with('!')
}

/// Glob text after stripping a leading `!`, if present.
fn glob_text(value: &str) -> &str {
    value.strip_prefix('!').unwrap_or(value)
}

pub(crate) fn evaluate(
    rule: &ResolvedRule, hint: &RuleHint, model: &WorkspaceModel,
) -> Result<Vec<PathBuf>, HintError> {
    let glob = glob_text(&hint.value);
    let pattern = Pattern::new(glob).map_err(|_silenced| HintError::Unsupported {
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

#[cfg(test)]
mod unit {
    use super::*;
    use crate::lint::{File, FileKind, ScanProfile, WorkspaceModel, WorkspaceModelVersion};
    use crate::rules::{HintKind, Origin, PathRoot, ResolvedRule, RuleHint};

    fn model_with_paths(paths: &[&str]) -> WorkspaceModel {
        WorkspaceModel {
            version: WorkspaceModelVersion,
            project_dir: "/tmp".to_string(),
            scan_profile: ScanProfile::Framework,
            artifact_paths: vec![],
            languages: vec![],
            files: paths
                .iter()
                .map(|p| File {
                    path: (*p).to_string(),
                    kind: FileKind::Text,
                    language: Some("markdown".to_string()),
                    sha256: None,
                })
                .collect(),
            frontmatter: vec![],
            markdown_sections: vec![],
            markdown_links: vec![],
            symlinks: vec![],
            skills: vec![],
            adapter_manifests: vec![],
            marketplace_entries: vec![],
            rule_index: vec![],
            text_matches: vec![],
            ignore_directives: vec![],
            briefs: vec![],
            fenced_blocks: vec![],
            scenarios: vec![],
            adapter_dirs: vec![],
        }
    }

    fn rule() -> ResolvedRule {
        ResolvedRule {
            rule_id: "TEST".to_string(),
            title: "test".to_string(),
            severity: specify_diagnostics::Severity::Important,
            trigger: String::new(),
            lint_mode: None,
            applicability: None,
            rule_hints: None,
            references: None,
            origin: Origin::Core,
            path_root: PathRoot::RulesRoot,
            path: "adapters/shared/rules/core/TEST.md".to_string(),
            body: String::new(),
            deprecated: None,
        }
    }

    fn path_hint(value: &str) -> RuleHint {
        RuleHint {
            kind: HintKind::PathPattern,
            value: value.to_string(),
            description: None,
            config: None,
        }
    }

    #[test]
    fn exclusion_prefix_matches_paths() {
        let model = model_with_paths(&["docs/a.md", "docs/explanation/decision-log.md"]);
        let hint = path_hint("!docs/explanation/decision-log.md");
        let matched = evaluate(&rule(), &hint, &model).expect("evaluate");
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].to_string_lossy(), "docs/explanation/decision-log.md");
    }

    #[test]
    fn include_pattern_unchanged() {
        let model = model_with_paths(&["a.rs", "b.md"]);
        let hint = path_hint("*.rs");
        let matched = evaluate(&rule(), &hint, &model).expect("evaluate");
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].to_string_lossy(), "a.rs");
    }
}
