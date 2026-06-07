//! Hint interpreter umbrella per the executable hint-kind contract
//! and §"Evaluation algorithm".
//!
//! v1 (Phase 2) ships the four executable hint kinds the contract lists
//! ([`HintKind::PathPattern`], [`HintKind::Schema`], [`HintKind::Regex`],
//! [`HintKind::Tool`]). The framework-convergence
//! family adds [`HintKind::ReferenceResolves`], [`HintKind::Unique`],
//! [`HintKind::SetCoverage`], [`HintKind::Cardinality`],
//! [`HintKind::ConstantEq`], [`HintKind::SetEq`],
//! and [`HintKind::ContentDigestEq`] in
//! the same family. Each rule's
//! hints are partitioned by kind and evaluated in the fixed order
//! `path-pattern → schema → reference-resolves → unique → set-coverage → cardinality → constant-eq → set-eq → content-digest-eq → fenced-block → regex → tool`
//! so the cheap filters narrow the candidate file set before the
//! subprocess boundary fires.
//!
//! When a rule carries multiple include `path-pattern` hints they UNION.
//! Hints whose `value` starts with `!` are exclusions applied after the
//! include union. When a rule carries only exclusions,
//! the starting set is every file in the model. When a rule carries zero
//! `path-pattern` hints the
//! candidate set defaults to every [`crate::lint::File`] in
//! [`crate::lint::WorkspaceModel`]; per-kind sub-evaluators apply
//! their own [`crate::lint::FileKind`] filter (e.g. regex skips
//! binaries).
//!
//! Every [`HintKind`] variant has an executable interpreter and the
//! partition is exhaustive over executable arms — no hint kind is
//! reserved.
//!
//! # Evidence cap (the structured evidence union)
//!
//! Every finding minted here passes through
//! [`specify_diagnostics::validate_evidence_size`] before `compute_fingerprint`
//! signs it. Snippet-evidence findings that exceed the 16 `KiB` cap are
//! truncated by halving the snippet value (clamped to a UTF-8 char
//! boundary) and appending a `…[truncated]` marker until the
//! serialised evidence object fits, then re-fingerprinted. Structured
//! evidence too large to inline collapses to
//! `{"truncated": true}`. Findings with [`specify_diagnostics::FindingEvidence::Digest`]
//! evidence above the cap are not produced by v1 evaluators; the
//! truncation loop bails on them rather than synthesising a bogus
//! payload.

pub mod cardinality;
pub mod constant_eq;
pub mod content_digest_eq;
mod error;
pub mod fenced_block;
mod finding;
pub mod path_pattern;
pub mod reference_resolves;
pub mod regex;
pub mod schema;
pub mod set_coverage;
pub mod set_eq;
pub mod tool;
pub mod unique;

use std::path::{Path, PathBuf};

pub use error::HintError;
pub(crate) use finding::{SyntheticFinding, make_finding, make_synthetic_finding, restamp_finding};
use specify_diagnostics::Diagnostic;
use specify_error::Error as CliError;
pub use tool::{ToolOutput, ToolRunError, ToolRunner};

use crate::lint::WorkspaceModel;
use crate::lint::diagnostics::map_hint_error;
use crate::rules::{HintKind, LintMode, ResolvedRule, RuleHint};

/// Per-rule output of [`evaluate`].
#[derive(Debug, Clone)]
pub struct HintEvalOutcome {
    /// Findings minted for this rule's executable hints.
    pub findings: Vec<Diagnostic>,
    /// Finding-id counter passed into the next [`evaluate`] call so
    /// `FIND-NNNN` ids stay monotonic across rules in the same scan.
    pub next_id_counter: u64,
}

/// Evaluate a single rule's hints against the workspace model.
///
/// Hints are partitioned by kind and run in the order
/// `path-pattern → schema → reference-resolves → unique → set-coverage → cardinality → constant-eq → set-eq → content-digest-eq → fenced-block → regex → tool`
/// per §"Evaluation algorithm".
/// `path-pattern` hits build the candidate file set the later kinds
/// consume.
///
/// `start_id_counter` seeds the `FIND-NNNN` id sequence; the caller
/// threads [`HintEvalOutcome::next_id_counter`] into the next call so
/// ids stay monotonic across rules.
///
/// # Errors
///
/// Any [`HintError`] variant — see the per-variant docs.
pub fn evaluate(
    rule: &ResolvedRule, hints: &[RuleHint], model: &WorkspaceModel, project_dir: &Path,
    tool_runner: &dyn ToolRunner, start_id_counter: u64,
) -> Result<HintEvalOutcome, HintError> {
    let mut schema_cache = schema::SchemaCache::default();
    evaluate_with_cache(
        rule,
        hints,
        model,
        project_dir,
        tool_runner,
        start_id_counter,
        &mut schema_cache,
    )
}

/// [`evaluate`] threaded with a caller-owned [`schema::SchemaCache`] so a
/// `kind: schema` validator (and its resolved project path) is built once
/// per lint run rather than once per rule. [`evaluate_rules`] owns the
/// run-scoped cache; the standalone [`evaluate`] entry point passes a
/// fresh per-call cache (behaviour is identical either way — the cache
/// only elides recompilation).
fn evaluate_with_cache(
    rule: &ResolvedRule, hints: &[RuleHint], model: &WorkspaceModel, project_dir: &Path,
    tool_runner: &dyn ToolRunner, start_id_counter: u64, schema_cache: &mut schema::SchemaCache,
) -> Result<HintEvalOutcome, HintError> {
    let mut findings: Vec<Diagnostic> = Vec::new();
    let mut next_id = start_id_counter;

    let mut path_pattern_hints: Vec<&RuleHint> = Vec::new();
    let mut schema_hints: Vec<&RuleHint> = Vec::new();
    let mut reference_resolves_hints: Vec<&RuleHint> = Vec::new();
    let mut unique_hints: Vec<&RuleHint> = Vec::new();
    let mut set_coverage_hints: Vec<&RuleHint> = Vec::new();
    let mut cardinality_hints: Vec<&RuleHint> = Vec::new();
    let mut constant_eq_hints: Vec<&RuleHint> = Vec::new();
    let mut set_eq_hints: Vec<&RuleHint> = Vec::new();
    let mut content_digest_eq_hints: Vec<&RuleHint> = Vec::new();
    let mut fenced_block_hints: Vec<&RuleHint> = Vec::new();
    let mut regex_hints: Vec<&RuleHint> = Vec::new();
    let mut tool_hints: Vec<&RuleHint> = Vec::new();

    for hint in hints {
        match hint.kind {
            HintKind::PathPattern => path_pattern_hints.push(hint),
            HintKind::Schema => schema_hints.push(hint),
            HintKind::ReferenceResolves => reference_resolves_hints.push(hint),
            HintKind::Unique => unique_hints.push(hint),
            HintKind::SetCoverage => set_coverage_hints.push(hint),
            HintKind::Cardinality => cardinality_hints.push(hint),
            HintKind::ConstantEq => constant_eq_hints.push(hint),
            HintKind::SetEq => set_eq_hints.push(hint),
            HintKind::ContentDigestEq => content_digest_eq_hints.push(hint),
            HintKind::FencedBlock => fenced_block_hints.push(hint),
            HintKind::Regex => regex_hints.push(hint),
            HintKind::Tool => tool_hints.push(hint),
        }
    }

    let candidates = build_candidate_set(rule, &path_pattern_hints, model)?;

    for hint in schema_hints {
        let mut new = schema::evaluate(
            rule,
            hint,
            &candidates,
            project_dir,
            model,
            &mut next_id,
            schema_cache,
        )?;
        findings.append(&mut new);
    }
    for hint in reference_resolves_hints {
        let mut new = reference_resolves::evaluate(rule, hint, &candidates, model, &mut next_id)?;
        findings.append(&mut new);
    }
    for hint in unique_hints {
        let mut new = unique::evaluate(rule, hint, &candidates, model, &mut next_id)?;
        findings.append(&mut new);
    }
    for hint in set_coverage_hints {
        let mut new = set_coverage::evaluate(rule, hint, &candidates, model, &mut next_id)?;
        findings.append(&mut new);
    }
    for hint in cardinality_hints {
        let mut new = cardinality::evaluate(rule, hint, &candidates, model, &mut next_id)?;
        findings.append(&mut new);
    }
    for hint in constant_eq_hints {
        let mut new = constant_eq::evaluate(rule, hint, &candidates, model, &mut next_id)?;
        findings.append(&mut new);
    }
    for hint in set_eq_hints {
        let mut new = set_eq::evaluate(rule, hint, &candidates, model, &mut next_id)?;
        findings.append(&mut new);
    }
    for hint in content_digest_eq_hints {
        let mut new = content_digest_eq::evaluate(rule, hint, &candidates, model, &mut next_id)?;
        findings.append(&mut new);
    }
    for hint in fenced_block_hints {
        let mut new = fenced_block::evaluate(rule, hint, &candidates, model, &mut next_id)?;
        findings.append(&mut new);
    }
    for hint in regex_hints {
        let mut new = regex::evaluate(rule, hint, &candidates, project_dir, model, &mut next_id)?;
        findings.append(&mut new);
    }
    for hint in tool_hints {
        let mut new =
            tool::evaluate(rule, hint, &candidates, project_dir, tool_runner, &mut next_id)?;
        findings.append(&mut new);
    }

    Ok(HintEvalOutcome {
        findings,
        next_id_counter: next_id,
    })
}

/// Fold [`evaluate`] over every rule, accumulating findings and the
/// `FIND-NNNN` id counter.
///
/// Shared by both lint surfaces (`specify lint product` and `specify lint framework`)
/// so the per-rule gating stays identical: rules in `lint-mode:
/// model-assisted` and rules with no (or empty) `rule_hints`
/// are skipped; `start_id` is threaded forward so ids stay monotonic.
///
/// `rule_filter` is the operator's allow-list: EMPTY means no filtering
/// (runtime), non-empty keeps only rules whose `rule_id` matches
/// verbatim (the `specify lint framework --rules` surface — exact, case-sensitive).
///
/// Per-rule [`HintError`]s are mapped through [`map_hint_error`] here so
/// both call sites collapse to one fallible call.
///
/// # Errors
///
/// The [`map_hint_error`] mapping of the first rule whose [`evaluate`]
/// call fails.
pub fn evaluate_rules(
    rules: &[ResolvedRule], model: &WorkspaceModel, project_dir: &Path, runner: &dyn ToolRunner,
    start_id: u64, rule_filter: &[&str],
) -> Result<(Vec<Diagnostic>, u64), CliError> {
    let mut findings: Vec<Diagnostic> = Vec::new();
    let mut next_id = start_id;
    // One cache per run: a schema referenced by many rules compiles once.
    let mut schema_cache = schema::SchemaCache::default();

    for rule in rules {
        if !rule_filter.is_empty() && !rule_filter.contains(&rule.rule_id.as_str()) {
            continue;
        }
        // `lint-mode: model-assisted` rules carry no executable hints
        // the deterministic engine can score. Rather than silently
        // dropping them, surface each as a non-blocking `kind: review`
        // diagnostic so the "needs judgment" signal stays first-class
        // on the wire (the model scorer / human reviewer picks it up).
        if matches!(rule.lint_mode, Some(LintMode::ModelAssisted)) {
            findings.push(finding::make_review_finding(rule, next_id));
            next_id += 1;
            continue;
        }
        let Some(hints) = rule.rule_hints.as_deref() else {
            continue;
        };
        if hints.is_empty() {
            continue;
        }
        let outcome = evaluate_with_cache(
            rule,
            hints,
            model,
            project_dir,
            runner,
            next_id,
            &mut schema_cache,
        )
        .map_err(|err| map_hint_error(rule, err))?;
        findings.extend(outcome.findings);
        next_id = outcome.next_id_counter;
    }

    Ok((findings, next_id))
}

fn build_candidate_set(
    rule: &ResolvedRule, path_pattern_hints: &[&RuleHint], model: &WorkspaceModel,
) -> Result<Vec<PathBuf>, HintError> {
    if path_pattern_hints.is_empty() {
        let mut paths: Vec<PathBuf> = model.files.iter().map(|f| PathBuf::from(&f.path)).collect();
        paths.sort();
        return Ok(paths);
    }

    let (excludes, includes): (Vec<&RuleHint>, Vec<&RuleHint>) =
        path_pattern_hints.iter().copied().partition(|hint| path_pattern::is_exclusion(hint));

    let mut set: std::collections::BTreeSet<PathBuf> = std::collections::BTreeSet::new();
    if includes.is_empty() {
        for file in &model.files {
            set.insert(PathBuf::from(&file.path));
        }
    } else {
        for hint in &includes {
            for path in path_pattern::evaluate(rule, hint, model)? {
                set.insert(path);
            }
        }
    }
    for hint in &excludes {
        for path in path_pattern::evaluate(rule, hint, model)? {
            set.remove(&path);
        }
    }
    Ok(set.into_iter().collect())
}

/// Render the candidate `PathBuf` slice into the `/`-relative string
/// set the fact-iterating sub-evaluators test membership against.
///
/// Every fact-iterating kind (`set-coverage`, `set-eq`, `constant-eq`,
/// `reference-resolves`, `unique`, `cardinality`)
/// narrows its facts to the `path-pattern` candidate set by string
/// path. Sharing the conversion keeps that lookup identical across
/// kinds.
pub(crate) fn candidate_set(candidates: &[PathBuf]) -> std::collections::BTreeSet<String> {
    candidates.iter().map(|p| p.to_string_lossy().into_owned()).collect()
}
