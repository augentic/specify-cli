//! Hint interpreter umbrella per the executable hint-kind contract
//! and §"Evaluation algorithm".
//!
//! v1 (Phase 2) ships the four executable hint kinds the contract lists
//! ([`HintKind::PathPattern`], [`HintKind::Schema`], [`HintKind::Regex`],
//! [`HintKind::Tool`]) plus the reserved-hint diagnostics reserved-kind summary policy
//! (`review.reserved-hint-skipped`). The framework-convergence
//! family adds [`HintKind::ReferenceResolves`], [`HintKind::Unique`],
//! [`HintKind::SetCoverage`], [`HintKind::Cardinality`],
//! [`HintKind::ConstantEq`], [`HintKind::SetEq`],
//! [`HintKind::ContentDigestEq`], and [`HintKind::NamespaceOwner`] in
//! the same family. Each rule's
//! hints are partitioned by kind and evaluated in the fixed order
//! `path-pattern → schema → reference-resolves → unique → set-coverage → cardinality → constant-eq → set-eq → content-digest-eq → namespace-owner → regex → tool`
//! so the cheap filters narrow the candidate file set before the
//! subprocess boundary fires.
//!
//! When a rule carries multiple `path-pattern` hints they UNION — a
//! file is a candidate when it matches any of the supplied patterns —
//! so authors can list independent globs without writing a single
//! brace-expansion. When a rule carries zero `path-pattern` hints the
//! candidate set defaults to every [`crate::lint::File`] in
//! [`crate::lint::WorkspaceModel`]; per-kind sub-evaluators apply
//! their own [`crate::lint::FileKind`] filter (e.g. regex skips
//! binaries).
//!
//! # Reserved-kind policy (reserved-hint diagnostics)
//!
//! After C17 NO hint kind is reserved — every [`HintKind`] variant has
//! an executable interpreter and the partition above is exhaustive
//! over executable arms. The reserved-kind machinery
//! ([`ReservedSkipped`], [`HintEvalOutcome::reserved_skipped`],
//! [`reserved_hint_summary`], the `review.reserved-hint-skipped`
//! finding) stays as forward-compat scaffolding: a future kind landed
//! as reserved before its interpreter would push a [`ReservedSkipped`]
//! entry, the caller would accumulate the entries across every rule it
//! evaluates, and [`reserved_hint_summary`] would fold them into a
//! single `review.reserved-hint-skipped` summary finding (strict mode
//! upgrades the severity from `optional` to `important`; the `rule_id`
//! is the same in both modes so dashboards aggregate across strict /
//! non-strict runs). With no reserved kind present,
//! [`HintEvalOutcome::reserved_skipped`] is always empty in real runs.
//!
//! # Evidence cap (the structured evidence union)
//!
//! Every finding minted here passes through
//! [`crate::rules::validate_evidence_size`] before `compute_fingerprint`
//! signs it. Snippet-evidence findings that exceed the 16 `KiB` cap are
//! truncated by halving the snippet value (clamped to a UTF-8 char
//! boundary) and appending a `…[truncated]` marker until the
//! serialised evidence object fits, then re-fingerprinted. Structured
//! evidence too large to inline collapses to
//! `{"truncated": true}`. Findings with [`crate::rules::FindingEvidence::Digest`]
//! evidence above the cap are not produced by v1 evaluators; the
//! truncation loop bails on them rather than synthesising a bogus
//! payload.

pub mod cardinality;
pub mod constant_eq;
pub mod content_digest_eq;
pub mod namespace_owner;
pub mod path_pattern;
pub mod reference_resolves;
pub mod regex;
pub mod schema;
pub mod set_coverage;
pub mod set_eq;
pub mod tool;
pub mod unique;

use std::path::{Path, PathBuf};

use specify_diagnostics::{
    Artifact, Confidence, Diagnostic, DiagnosticKind, DiagnosticSource, FindingEvidence,
    FindingLocation, Severity, fingerprint as compute_fingerprint, validate_evidence_size,
};
use specify_error::Error as CliError;
use thiserror::Error;
pub use tool::{ToolOutput, ToolRunError, ToolRunner};

use crate::lint::WorkspaceModel;
use crate::lint::diagnostics::map_hint_error;
use crate::rules::{DeterministicHint, HintKind, LintMode, ResolvedRule};

/// Closed failure mode for the hint interpreter.
///
/// Variants map to the lint exit mapping exit-code table at the handler boundary —
/// `Unsupported`, `SchemaCompile`, `SchemaResolve`, `RegexCompile`,
/// `Filesystem`, and `ToolInvocation` are infrastructure failures
/// the caller maps to `Error::Validation` (exit 2) or
/// `Error::Filesystem` (exit 1) per lint exit mapping. Recoverable per-finding
/// states (`tool.invocation-failed`, `tool.undeclared`, the reserved-hint diagnostics
/// summary) flow back as [`Diagnostic`] entries on the Ok path.
#[derive(Debug, Error)]
pub enum HintError {
    /// Hint shape outside the v1 contract (reserved kinds called
    /// directly, `http(s)://` schema refs, glob negation, …).
    #[error("rule {rule_id}: hint kind {kind:?} unsupported: {reason}")]
    Unsupported {
        /// Originating rule id.
        rule_id: String,
        /// Hint kind that triggered the rejection.
        kind: HintKind,
        /// Static reason copied into operator-facing diagnostics.
        reason: &'static str,
    },
    /// JSON Schema referenced by a `kind: schema` hint failed to
    /// compile.
    #[error("rule {rule_id}: schema {schema_ref} failed to compile: {detail}")]
    SchemaCompile {
        /// Originating rule id.
        rule_id: String,
        /// Schema reference verbatim from the hint's `value`.
        schema_ref: String,
        /// Compiler error message.
        detail: String,
    },
    /// Schema reference could not be resolved (unknown registered id,
    /// missing project file, escapes `project_dir` via `..`,
    /// `http(s)://` ref).
    #[error("rule {rule_id}: schema {schema_ref} could not be resolved: {reason}")]
    SchemaResolve {
        /// Originating rule id.
        rule_id: String,
        /// Schema reference verbatim from the hint's `value`.
        schema_ref: String,
        /// Free-form resolution reason.
        reason: String,
    },
    /// Regex pattern carried by a `kind: regex` hint did not compile.
    #[error("rule {rule_id}: regex {pattern} failed to compile: {source}")]
    RegexCompile {
        /// Originating rule id.
        rule_id: String,
        /// Pattern verbatim from the hint's `value`.
        pattern: String,
        /// Underlying `regex` crate error.
        #[source]
        source: ::regex::Error,
    },
    /// Tool invocation failed at the runtime boundary (the WASI host
    /// could not invoke the declared tool). Recoverable
    /// non-zero-exit outcomes flow as `tool.invocation-failed`
    /// findings on the Ok path per `kind: tool` evaluator contract.
    #[error("rule {rule_id}: tool {tool} invocation failed: {detail}")]
    ToolInvocation {
        /// Originating rule id.
        rule_id: String,
        /// Tool name from the hint's `value`.
        tool: String,
        /// Free-form invocation failure detail.
        detail: String,
    },
    /// Reserved variant — `tool.undeclared` is emitted as a finding
    /// on the Ok path per `kind: tool` evaluator contract. The variant is preserved on the
    /// closed enum so callers exhaustively match every `kind: tool` evaluator contract-mandated
    /// surface.
    #[error("rule {rule_id}: tool {tool} not declared by the project")]
    ToolUndeclared {
        /// Originating rule id.
        rule_id: String,
        /// Tool name from the hint's `value`.
        tool: String,
    },
    /// Filesystem I/O against a candidate file failed during
    /// evaluation (the indexer normally skips unreadable files but
    /// races between scan and eval can still surface here).
    #[error("filesystem {op} on {path}: {source}", path = path.display())]
    Filesystem {
        /// Operation name (`read`, `parse`, …).
        op: &'static str,
        /// Path the operation targeted.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
}

/// One reserved-kind hint occurrence captured per call to [`evaluate`].
///
/// Caller accumulates [`ReservedSkipped`] entries across every rule
/// in the scan and feeds the aggregate to [`reserved_hint_summary`]
/// to mint the single reserved-hint diagnostics summary finding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReservedSkipped {
    /// Rule that carried the reserved hint.
    pub rule_id: String,
    /// Index of the hint inside the rule's `deterministic_hints`
    /// list (0-based).
    pub hint_index: usize,
    /// The reserved kind that was skipped.
    pub kind: HintKind,
}

/// Per-rule output of [`evaluate`].
#[derive(Debug, Clone)]
pub struct HintEvalOutcome {
    /// Findings minted for this rule's executable hints.
    pub findings: Vec<Diagnostic>,
    /// Reserved-kind hint occurrences encountered while iterating
    /// the rule's hints.
    pub reserved_skipped: Vec<ReservedSkipped>,
    /// Finding-id counter passed into the next [`evaluate`] call so
    /// `FIND-NNNN` ids stay monotonic across rules in the same scan.
    pub next_id_counter: u64,
}

/// Evaluate a single rule's hints against the workspace model.
///
/// Hints are partitioned by kind and run in the order
/// `path-pattern → schema → reference-resolves → unique → set-coverage → cardinality → constant-eq → set-eq → content-digest-eq → namespace-owner → regex → tool`
/// per §"Evaluation algorithm".
/// `path-pattern` hits build the candidate file set the later kinds
/// consume. No hint kind is reserved after C17, so
/// [`HintEvalOutcome::reserved_skipped`] stays empty in real runs; the
/// field remains for forward-compat with any future reserved kind.
///
/// `start_id_counter` seeds the `FIND-NNNN` id sequence; the caller
/// threads [`HintEvalOutcome::next_id_counter`] into the next call so
/// ids stay monotonic across rules.
///
/// # Errors
///
/// Any [`HintError`] variant — see the per-variant docs.
pub fn evaluate(
    rule: &ResolvedRule, hints: &[DeterministicHint], model: &WorkspaceModel, project_dir: &Path,
    tool_runner: &dyn ToolRunner, start_id_counter: u64,
) -> Result<HintEvalOutcome, HintError> {
    let mut findings: Vec<Diagnostic> = Vec::new();
    // No hint kind is reserved after C17; the machinery stays as
    // forward-compat scaffolding for any future kind landed reserved.
    let reserved_skipped: Vec<ReservedSkipped> = Vec::new();
    let mut next_id = start_id_counter;

    let mut path_pattern_hints: Vec<&DeterministicHint> = Vec::new();
    let mut schema_hints: Vec<&DeterministicHint> = Vec::new();
    let mut reference_resolves_hints: Vec<&DeterministicHint> = Vec::new();
    let mut unique_hints: Vec<&DeterministicHint> = Vec::new();
    let mut set_coverage_hints: Vec<&DeterministicHint> = Vec::new();
    let mut cardinality_hints: Vec<&DeterministicHint> = Vec::new();
    let mut constant_eq_hints: Vec<&DeterministicHint> = Vec::new();
    let mut set_eq_hints: Vec<&DeterministicHint> = Vec::new();
    let mut content_digest_eq_hints: Vec<&DeterministicHint> = Vec::new();
    let mut namespace_owner_hints: Vec<&DeterministicHint> = Vec::new();
    let mut regex_hints: Vec<&DeterministicHint> = Vec::new();
    let mut tool_hints: Vec<&DeterministicHint> = Vec::new();

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
            HintKind::NamespaceOwner => namespace_owner_hints.push(hint),
            HintKind::Regex => regex_hints.push(hint),
            HintKind::Tool => tool_hints.push(hint),
        }
    }

    let candidates = build_candidate_set(rule, &path_pattern_hints, model)?;

    for hint in schema_hints {
        let mut new = schema::evaluate(rule, hint, &candidates, project_dir, model, &mut next_id)?;
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
    for hint in namespace_owner_hints {
        let mut new = namespace_owner::evaluate(rule, hint, &candidates, model, &mut next_id)?;
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
        reserved_skipped,
        next_id_counter: next_id,
    })
}

/// Fold [`evaluate`] over every rule, accumulating findings, reserved
/// skips, and the `FIND-NNNN` id counter.
///
/// Shared by both lint surfaces (`specrun lint run` and `specdev lint`)
/// so the per-rule gating stays identical: rules in `lint-mode:
/// model-assisted` and rules with no (or empty) `deterministic_hints`
/// are skipped; `start_id` is threaded forward so ids stay monotonic.
///
/// `rule_filter` is the operator's allow-list: EMPTY means no filtering
/// (runtime), non-empty keeps only rules whose `rule_id` matches
/// verbatim (the `specdev lint --rules` surface — exact, case-sensitive).
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
) -> Result<(Vec<Diagnostic>, Vec<ReservedSkipped>, u64), CliError> {
    let mut findings: Vec<Diagnostic> = Vec::new();
    let mut reserved: Vec<ReservedSkipped> = Vec::new();
    let mut next_id = start_id;

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
            findings.push(make_review_finding(rule, next_id));
            next_id += 1;
            continue;
        }
        let Some(hints) = rule.deterministic_hints.as_deref() else {
            continue;
        };
        if hints.is_empty() {
            continue;
        }
        let outcome = evaluate(rule, hints, model, project_dir, runner, next_id)
            .map_err(|err| map_hint_error(rule, err))?;
        findings.extend(outcome.findings);
        reserved.extend(outcome.reserved_skipped);
        next_id = outcome.next_id_counter;
    }

    Ok((findings, reserved, next_id))
}

/// Mint the reserved-hint diagnostics reserved-hint summary finding from accumulated
/// [`ReservedSkipped`] entries, or return `None` when the input is
/// empty.
///
/// `strict_hints` upgrades the finding's severity from `optional` to
/// `important`; the `rule_id` is `review.reserved-hint-skipped` in
/// both modes per reserved-hint diagnostics.
#[must_use]
pub fn reserved_hint_summary(
    skipped: &[ReservedSkipped], strict_hints: bool,
) -> Option<Diagnostic> {
    if skipped.is_empty() {
        return None;
    }
    let pairs: Vec<serde_json::Value> =
        skipped.iter().map(|s| serde_json::json!([s.rule_id, s.hint_index])).collect();
    let evidence = FindingEvidence::Structured {
        summary: "Reserved hint kinds awaiting implementation".to_string(),
        data: serde_json::json!({ "pairs": pairs }),
        locations: None,
    };
    let severity = if strict_hints { Severity::Important } else { Severity::Optional };
    let mut finding = Diagnostic {
        id: "FIND-RESERVED".to_string(),
        rule_id: Some("review.reserved-hint-skipped".to_string()),
        related_rule_ids: None,
        title: "Reserved hint kinds awaiting implementation".to_string(),
        severity,
        source: DiagnosticSource::Deterministic,
        kind: DiagnosticKind::Violation,
        target_adapter: None,
        source_adapter: None,
        slice: None,
        change: None,
        artifact: Artifact::Unknown,
        location: None,
        evidence,
        impact: format!("{} reserved hint occurrence(s) skipped this scan.", skipped.len()),
        remediation: "Implement the reserved hint kind or remove the hint until support lands."
            .to_string(),
        confidence: Some(Confidence::High),
        fingerprint: String::new(),
        status: None,
        disposition: None,
    };
    clamp_evidence(&mut finding);
    finding.fingerprint = compute_fingerprint(&finding);
    Some(finding)
}

fn build_candidate_set(
    rule: &ResolvedRule, path_pattern_hints: &[&DeterministicHint], model: &WorkspaceModel,
) -> Result<Vec<PathBuf>, HintError> {
    if path_pattern_hints.is_empty() {
        let mut paths: Vec<PathBuf> = model.files.iter().map(|f| PathBuf::from(&f.path)).collect();
        paths.sort();
        return Ok(paths);
    }
    let mut set: std::collections::BTreeSet<PathBuf> = std::collections::BTreeSet::new();
    for hint in path_pattern_hints {
        for path in path_pattern::evaluate(rule, hint, model)? {
            set.insert(path);
        }
    }
    Ok(set.into_iter().collect())
}

/// Build a finding from rule-derived defaults (severity, target
/// adapter, impact, remediation), apply the §"Evidence cap"
/// truncation, and stamp the structured lint finding fingerprint.
pub(crate) fn make_finding(
    rule: &ResolvedRule, id_num: u64, title: String, location: Option<FindingLocation>,
    evidence: FindingEvidence,
) -> Diagnostic {
    let mut finding = Diagnostic {
        id: format!("FIND-{id_num:04}"),
        rule_id: Some(rule.rule_id.clone()),
        related_rule_ids: None,
        title,
        severity: rule.severity,
        source: DiagnosticSource::Deterministic,
        kind: DiagnosticKind::Violation,
        target_adapter: single_adapter(rule),
        source_adapter: None,
        slice: None,
        change: None,
        artifact: Artifact::Code,
        location,
        evidence,
        impact: rule.trigger.clone(),
        remediation: format!("See {}", rule.path),
        confidence: Some(Confidence::High),
        fingerprint: String::new(),
        status: None,
        disposition: None,
    };
    clamp_evidence(&mut finding);
    finding.fingerprint = compute_fingerprint(&finding);
    finding
}

/// Build a non-blocking `kind: review` diagnostic for a
/// `lint-mode: model-assisted` rule the deterministic engine cannot
/// score. The rule's `trigger` becomes the review prompt (impact +
/// snippet evidence) and its `path` the remediation pointer. Source is
/// `model-assisted` — the question is destined for a scorer, not a
/// deterministic verdict.
fn make_review_finding(rule: &ResolvedRule, id_num: u64) -> Diagnostic {
    let mut finding = Diagnostic {
        id: format!("FIND-{id_num:04}"),
        rule_id: Some(rule.rule_id.clone()),
        related_rule_ids: None,
        title: rule.title.clone(),
        severity: rule.severity,
        source: DiagnosticSource::ModelAssisted,
        kind: DiagnosticKind::Review,
        target_adapter: single_adapter(rule),
        source_adapter: None,
        slice: None,
        change: None,
        artifact: Artifact::Code,
        location: None,
        evidence: FindingEvidence::Snippet {
            value: rule.trigger.clone(),
        },
        impact: rule.trigger.clone(),
        remediation: format!("Model-assisted review required; see {}", rule.path),
        confidence: Some(Confidence::Medium),
        fingerprint: String::new(),
        status: None,
        disposition: None,
    };
    clamp_evidence(&mut finding);
    finding.fingerprint = compute_fingerprint(&finding);
    finding
}

/// Inputs for [`make_synthetic_finding`].
///
/// Named fields keep the synthetic-finding call sites readable: the
/// `tool.undeclared` / `tool.invocation-failed` shapes pass several
/// optional values (`location`, `target_adapter`) that would otherwise
/// be bare positional `None`s.
pub(crate) struct SyntheticFinding<'a> {
    /// Monotonic finding number rendered into the `FIND-NNNN` id.
    pub id_num: u64,
    /// Explicit rule id stamped on the finding.
    pub rule_id: &'a str,
    /// Human-readable finding title.
    pub title: String,
    /// Finding severity.
    pub severity: Severity,
    /// Optional source location.
    pub location: Option<FindingLocation>,
    /// Structured evidence payload.
    pub evidence: FindingEvidence,
    /// Impact line.
    pub impact: String,
    /// Remediation line.
    pub remediation: String,
    /// Optional owning target adapter.
    pub target_adapter: Option<String>,
}

/// Build a finding with an explicit `rule_id` / `severity` (for the
/// synthetic `tool.undeclared` and `tool.invocation-failed` shapes).
pub(crate) fn make_synthetic_finding(spec: SyntheticFinding<'_>) -> Diagnostic {
    let SyntheticFinding {
        id_num,
        rule_id,
        title,
        severity,
        location,
        evidence,
        impact,
        remediation,
        target_adapter,
    } = spec;
    let mut finding = Diagnostic {
        id: format!("FIND-{id_num:04}"),
        rule_id: Some(rule_id.to_string()),
        related_rule_ids: None,
        title,
        severity,
        source: DiagnosticSource::Deterministic,
        kind: DiagnosticKind::Violation,
        target_adapter,
        source_adapter: None,
        slice: None,
        change: None,
        artifact: Artifact::Code,
        location,
        evidence,
        impact,
        remediation,
        confidence: Some(Confidence::High),
        fingerprint: String::new(),
        status: None,
        disposition: None,
    };
    clamp_evidence(&mut finding);
    finding.fingerprint = compute_fingerprint(&finding);
    finding
}

/// Stamp `id` and recompute the fingerprint on a finding produced
/// outside the rule-derived defaults (e.g. forwarded from a tool's
/// stdout). Applies the evidence-cap truncation before signing.
pub(crate) fn restamp_finding(finding: &mut Diagnostic, id_num: u64) {
    finding.id = format!("FIND-{id_num:04}");
    clamp_evidence(finding);
    finding.fingerprint = compute_fingerprint(finding);
}

fn single_adapter(rule: &ResolvedRule) -> Option<String> {
    let adapters = rule.applicability.as_ref().and_then(|a| a.adapters.as_ref())?;
    if adapters.len() != 1 {
        return None;
    }
    let raw = adapters[0].as_str();
    Some(raw.split_once('@').map_or_else(|| raw.to_owned(), |(name, _)| name.to_owned()))
}

const TRUNCATION_MARKER: &str = "…[truncated]";
const CLAMP_ITERATION_LIMIT: usize = 32;

fn clamp_evidence(finding: &mut Diagnostic) {
    let mut iter = 0;
    while validate_evidence_size(finding).is_err() && iter < CLAMP_ITERATION_LIMIT {
        iter += 1;
        match &mut finding.evidence {
            FindingEvidence::Snippet { value } => {
                if value.is_empty() {
                    break;
                }
                let target = value.len() / 2;
                let mut cut = target;
                while cut > 0 && !value.is_char_boundary(cut) {
                    cut -= 1;
                }
                value.truncate(cut);
                value.push_str(TRUNCATION_MARKER);
            }
            FindingEvidence::Structured { data, locations, .. } => {
                *data = serde_json::json!({ "truncated": true });
                *locations = None;
            }
            FindingEvidence::Digest { .. } => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::{Origin, PathRoot};

    fn rule(adapters: Option<Vec<String>>) -> ResolvedRule {
        ResolvedRule {
            rule_id: "UNI-001".into(),
            title: "t".into(),
            severity: Severity::Important,
            trigger: "trigger".into(),
            lint_mode: None,
            applicability: adapters.map(|a| crate::rules::Applicability {
                adapters: Some(a),
                languages: None,
                artifacts: None,
                paths: None,
            }),
            deterministic_hints: None,
            references: None,
            origin: Origin::Shared,
            path_root: PathRoot::RulesRoot,
            path: "shared/UNI-001.md".into(),
            body: String::new(),
            deprecated: None,
        }
    }

    #[test]
    fn single_adapter_strips_version_suffix() {
        let r = rule(Some(vec!["omnia@v2".into()]));
        assert_eq!(single_adapter(&r).as_deref(), Some("omnia"));
    }

    #[test]
    fn single_adapter_none_when_multiple() {
        let r = rule(Some(vec!["omnia".into(), "vectis".into()]));
        assert!(single_adapter(&r).is_none());
    }

    #[test]
    fn reserved_summary_empty_when_none() {
        assert!(reserved_hint_summary(&[], false).is_none());
    }

    #[test]
    fn reserved_summary_stable_rule_id() {
        let skipped = vec![ReservedSkipped {
            rule_id: "UNI-099".into(),
            hint_index: 0,
            kind: HintKind::NamespaceOwner,
        }];
        let optional = reserved_hint_summary(&skipped, false).expect("present");
        let strict = reserved_hint_summary(&skipped, true).expect("present");
        assert_eq!(optional.rule_id.as_deref(), Some("review.reserved-hint-skipped"));
        assert_eq!(strict.rule_id.as_deref(), Some("review.reserved-hint-skipped"));
        assert_eq!(optional.severity, Severity::Optional);
        assert_eq!(strict.severity, Severity::Important);
    }

    #[test]
    fn clamp_truncates_oversize_snippet() {
        let mut finding = Diagnostic {
            id: "FIND-0001".into(),
            rule_id: Some("UNI-001".into()),
            related_rule_ids: None,
            title: "t".into(),
            severity: Severity::Important,
            source: DiagnosticSource::Deterministic,
            kind: DiagnosticKind::Violation,
            target_adapter: None,
            source_adapter: None,
            slice: None,
            change: None,
            artifact: Artifact::Code,
            location: None,
            evidence: FindingEvidence::Snippet {
                value: "a".repeat(64 * 1024),
            },
            impact: "i".into(),
            remediation: "r".into(),
            confidence: Some(Confidence::High),
            fingerprint: String::new(),
            status: None,
            disposition: None,
        };
        clamp_evidence(&mut finding);
        validate_evidence_size(&finding).expect("evidence fits within cap");
        if let FindingEvidence::Snippet { value } = &finding.evidence {
            assert!(value.ends_with(TRUNCATION_MARKER));
        } else {
            panic!("snippet variant preserved");
        }
    }
}
