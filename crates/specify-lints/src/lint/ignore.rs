//! RFC-33a directive-validation pass.
//!
//! Runs after hint evaluation and before envelope assembly. The pass
//! consumes [`crate::lint::WorkspaceModel::ignore_directives`] and the
//! current scan's finding set, stamps matching findings with
//! [`FindingStatus::Ignored`] (or [`FindingStatus::FalsePositive`]
//! when the directive's rationale carries the `false-positive:`
//! prefix), and mints synthetic `UNI-022` / `UNI-023` findings for
//! malformed or orphan directives per RFC-33a §"Directive-without-
//! rationale is a finding" (D4) and §"Implementation plan" step 4.
//!
//! # Graceful degradation
//!
//! Per RFC-33a §"Graceful degradation when the universal codex tree
//! is absent" the match-and-demote logic runs unconditionally;
//! synthetic `UNI-022` / `UNI-023` emission is suppressed when the
//! corresponding rule did not resolve in the current scan. Consumer
//! projects without the shared codex tree see directives applied
//! with `disposition.directive` populated but never trip the
//! synthetic findings.
//!
//! # Determinism
//!
//! The pass sorts directives by `(path, line, rule_id)` before
//! iterating so the stamping order and synthetic ordering are stable
//! irrespective of the indexer's emission order. Findings are
//! mutated in place; the returned synthetics are appended by the
//! runner before envelope assembly.
//!
//! # Status-aware exit decision
//!
//! [`blocking_findings_present`] implements the RFC-33a §"Exit and
//! presentation semantics" rule: exit 2 fires only when at least one
//! finding has `severity ∈ {critical, important}` AND `status` is
//! `open` (treating an unset status as `open`). The helper is kept
//! standalone so the lint runner and unit tests share one source of
//! truth for the decision.

use std::collections::HashMap;

use crate::lint::IgnoreDirective;
use crate::lint::eval::{SyntheticFinding, make_synthetic_finding};
use crate::rules::{
    DirectiveDisposition, DispositionSource, FindingDisposition, FindingEvidence, FindingLocation,
    FindingStatus, LintFinding, ResolvedRule, Severity,
};

/// Rationale prefix that demotes a matched finding to
/// [`FindingStatus::FalsePositive`] instead of
/// [`FindingStatus::Ignored`]. Case-sensitive per RFC-33a §"Finding
/// status taxonomy".
const FALSE_POSITIVE_PREFIX: &str = "false-positive:";

/// Minimum rationale length per RFC-33a §"Directive rationale stays
/// free-form" (D12). Shorter rationales parse cleanly but emit
/// [`UNI_022`].
const MIN_RATIONALE_LEN: usize = 16;

/// `UNI-022` — `ignore-directive-missing-rationale` per RFC-33a D13.
const UNI_022: &str = "UNI-022";

/// `UNI-023` — `ignore-directive-orphan` per RFC-33a D13.
const UNI_023: &str = "UNI-023";

/// Output of [`apply`].
///
/// Carries the synthetic `UNI-022` / `UNI-023` findings the runner
/// appends to the existing finding list plus the updated
/// `FIND-NNNN` counter so monotonic ids survive into any
/// post-directive emission (currently the reserved-hint summary uses
/// a fixed `FIND-RESERVED` id and does not consume the counter, but
/// keeping the threading explicit pre-empts a footgun if that
/// changes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IgnoreOutcome {
    /// Synthetic `UNI-022` / `UNI-023` findings to append to the
    /// envelope. Ordered by `(directive_path, directive_line,
    /// directive_rule_id, UNI-022 before UNI-023)` so envelope output
    /// stays byte-stable irrespective of indexer emission order.
    pub synthetics: Vec<LintFinding>,
    /// Next monotonic `FIND-NNNN` slot after minting the synthetics.
    pub next_id_counter: u64,
}

/// Apply the RFC-33a directive-validation pass to `findings`.
///
/// Walks `directives` in `(path, line, rule_id)` order, stamps every
/// finding whose `(path, line, rule_id)` matches with
/// [`FindingStatus::Ignored`] (or [`FindingStatus::FalsePositive`]
/// when the rationale begins with `false-positive:`) plus a
/// populated [`FindingDisposition::directive`], then mints synthetic
/// `UNI-022` / `UNI-023` findings per RFC-33a §"Implementation plan"
/// step 4.
///
/// `resolved_rules` carries severity metadata for the synthesised
/// findings; it also gates emission per RFC-33a §"Graceful
/// degradation when the universal codex tree is absent" — when
/// `UNI-022` / `UNI-023` are absent from the resolved set the
/// matching synthetic is suppressed silently.
///
/// `next_id` seeds the `FIND-NNNN` counter from the runner so
/// synthetic ids stay monotonic with the hint-evaluator output;
/// [`IgnoreOutcome::next_id_counter`] returns the post-mint counter.
pub fn apply(
    findings: &mut [LintFinding], directives: &[IgnoreDirective], resolved_rules: &[ResolvedRule],
    next_id: u64,
) -> IgnoreOutcome {
    let mut sorted: Vec<&IgnoreDirective> = directives.iter().collect();
    sorted.sort_by(|a, b| {
        a.path
            .cmp(&b.path)
            .then_with(|| a.line.cmp(&b.line))
            .then_with(|| a.rule_id.cmp(&b.rule_id))
    });

    let resolved_map: HashMap<&str, &ResolvedRule> =
        resolved_rules.iter().map(|r| (r.rule_id.as_str(), r)).collect();

    let mut matched_flags: Vec<bool> = vec![false; sorted.len()];

    for (idx, directive) in sorted.iter().enumerate() {
        for finding in findings.iter_mut() {
            if !finding_matches_directive(finding, directive) {
                continue;
            }
            matched_flags[idx] = true;
            // First directive to match a finding wins so stamping is
            // deterministic when two directives nominate the same
            // `(path, line, rule_id)`; both still count as matched
            // so orphan emission stays silent for either.
            if finding.disposition.is_some() {
                continue;
            }
            let status = if directive
                .rationale
                .as_deref()
                .is_some_and(|r| r.starts_with(FALSE_POSITIVE_PREFIX))
            {
                FindingStatus::FalsePositive
            } else {
                FindingStatus::Ignored
            };
            finding.status = Some(status);
            finding.disposition = Some(FindingDisposition {
                source: DispositionSource::Directive,
                directive: Some(DirectiveDisposition {
                    path: directive.path.clone(),
                    line: directive.line,
                    rationale: directive.rationale.clone().unwrap_or_default(),
                }),
                since: None,
            });
        }
    }

    let mut next = next_id;
    let mut synthetics: Vec<LintFinding> = Vec::new();
    for (idx, directive) in sorted.iter().enumerate() {
        let missing_rationale =
            directive.rationale.as_deref().is_none_or(|r| r.len() < MIN_RATIONALE_LEN);
        if missing_rationale && let Some(rule) = resolved_map.get(UNI_022) {
            synthetics.push(mint_synthetic(
                rule,
                directive,
                &mut next,
                "specify-ignore directive missing or short rationale".to_string(),
                format!(
                    "Directive for {rule_id} omits rationale or carries fewer than {MIN_RATIONALE_LEN} characters of rationale.",
                    rule_id = directive.rule_id,
                ),
                "Add a rationale of at least 16 characters explaining why the finding is being tolerated at this location.".to_string(),
            ));
        }

        if !matched_flags[idx]
            && let Some(rule) = resolved_map.get(UNI_023)
        {
            synthetics.push(mint_synthetic(
                rule,
                directive,
                &mut next,
                "specify-ignore directive does not match any finding".to_string(),
                format!(
                    "Directive references {rule_id} but no finding for that rule fires on the target line.",
                    rule_id = directive.rule_id,
                ),
                "Remove the directive, fix the rule id, or restore the underlying finding it was meant to suppress.".to_string(),
            ));
        }
    }

    IgnoreOutcome {
        synthetics,
        next_id_counter: next,
    }
}

/// Status-aware exit predicate per RFC-33a §"Exit and presentation
/// semantics".
///
/// Returns `true` when at least one finding has `severity ∈
/// {critical, important}` AND `status` of `open` (an unset `status`
/// is treated as `open` per the same section).
#[must_use]
pub fn blocking_findings_present(findings: &[LintFinding]) -> bool {
    findings.iter().any(is_blocking)
}

const fn is_blocking(finding: &LintFinding) -> bool {
    let blocking_severity = matches!(finding.severity, Severity::Critical | Severity::Important);
    let blocking_status = matches!(finding.status, None | Some(FindingStatus::Open));
    blocking_severity && blocking_status
}

fn finding_matches_directive(finding: &LintFinding, directive: &IgnoreDirective) -> bool {
    let Some(rule_id) = finding.rule_id.as_deref() else {
        return false;
    };
    if rule_id != directive.rule_id {
        return false;
    }
    let Some(location) = finding.location.as_ref() else {
        return false;
    };
    if location.path != directive.path {
        return false;
    }
    location.line == Some(directive.target_line)
}

fn mint_synthetic(
    rule: &ResolvedRule, directive: &IgnoreDirective, next_id: &mut u64, title: String,
    impact: String, remediation: String,
) -> LintFinding {
    let id = *next_id;
    *next_id = next_id.saturating_add(1);
    let location = FindingLocation {
        path: directive.path.clone(),
        line: Some(directive.line),
        column: None,
        end_line: None,
        end_column: None,
    };
    let evidence = FindingEvidence::Snippet {
        value: directive.raw.clone(),
    };
    let mut finding = make_synthetic_finding(SyntheticFinding {
        id_num: id,
        rule_id: rule.rule_id.as_str(),
        title,
        severity: rule.severity,
        location: Some(location),
        evidence,
        impact,
        remediation,
        target_adapter: None,
    });
    // RFC-33a §"Finding status taxonomy": synthetic directive
    // findings default to `open`; the runner's status-aware exit
    // decision treats `None` and `Some(Open)` equivalently but
    // stamping the explicit token keeps wire output unambiguous.
    finding.status = Some(FindingStatus::Open);
    finding
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::fingerprint::fingerprint as compute_fingerprint;
    use crate::rules::{
        Artifact, Confidence, FindingEvidence, FindingLocation, FindingSource, Origin, PathRoot,
        Severity,
    };

    fn directive(
        path: &str, line: u32, rule_id: &str, target_line: u32, rationale: Option<&str>,
    ) -> IgnoreDirective {
        let raw = rationale.map_or_else(
            || format!("// specify-ignore: {rule_id}"),
            |r| format!("// specify-ignore: {rule_id} — {r}"),
        );
        IgnoreDirective {
            path: path.into(),
            line,
            rule_id: rule_id.into(),
            rationale: rationale.map(str::to_string),
            target_line,
            raw,
        }
    }

    fn finding(rule_id: &str, path: &str, line: u32) -> LintFinding {
        let mut f = LintFinding {
            id: "FIND-0001".into(),
            rule_id: Some(rule_id.into()),
            related_rule_ids: None,
            title: "demo".into(),
            severity: Severity::Important,
            source: FindingSource::Deterministic,
            target_adapter: None,
            source_adapter: None,
            slice: None,
            change: None,
            artifact: Artifact::Code,
            location: Some(FindingLocation {
                path: path.into(),
                line: Some(line),
                column: None,
                end_line: None,
                end_column: None,
            }),
            evidence: FindingEvidence::Snippet {
                value: format!("payload for {rule_id}"),
            },
            impact: "i".into(),
            remediation: "r".into(),
            confidence: Some(Confidence::High),
            fingerprint: String::new(),
            status: None,
            disposition: None,
        };
        f.fingerprint = compute_fingerprint(&f);
        f
    }

    fn rule(rule_id: &str, severity: Severity) -> ResolvedRule {
        ResolvedRule {
            rule_id: rule_id.into(),
            title: format!("{rule_id} title"),
            severity,
            trigger: format!("Trigger for {rule_id}"),
            lint_mode: None,
            applicability: None,
            deterministic_hints: None,
            references: None,
            origin: Origin::Shared,
            path_root: PathRoot::RulesRoot,
            path: format!("shared/{rule_id}.md"),
            body: String::new(),
            deprecated: None,
        }
    }

    fn validation_rules() -> Vec<ResolvedRule> {
        vec![rule(UNI_022, Severity::Important), rule(UNI_023, Severity::Important)]
    }

    /// Test 1: a directive whose `(path, target_line, rule_id)`
    /// matches a finding flips it to `Ignored` and populates the
    /// directive disposition verbatim.
    #[test]
    fn match_demotes_finding_to_ignored() {
        let mut findings = vec![finding("UNI-014", "src/lib.rs", 18)];
        let dirs = vec![directive(
            "src/lib.rs",
            17,
            "UNI-014",
            18,
            Some("legitimate operator-acknowledged exception"),
        )];
        let outcome = apply(&mut findings, &dirs, &validation_rules(), 100);
        assert!(outcome.synthetics.is_empty(), "rationale is long enough; no synthetic expected");
        assert_eq!(outcome.next_id_counter, 100, "no synthetic minted; counter untouched");

        let stamped = &findings[0];
        assert_eq!(stamped.status, Some(FindingStatus::Ignored));
        let disp = stamped.disposition.as_ref().expect("disposition stamped");
        assert_eq!(disp.source, DispositionSource::Directive);
        let inner = disp.directive.as_ref().expect("directive sub-field");
        assert_eq!(inner.path, "src/lib.rs");
        assert_eq!(inner.line, 17);
        assert_eq!(inner.rationale, "legitimate operator-acknowledged exception");
        assert!(disp.since.is_none());
    }

    /// Test 2: rationale beginning with `false-positive:` demotes to
    /// `FalsePositive` instead of `Ignored`; the disposition still
    /// carries the verbatim rationale.
    #[test]
    fn false_positive_prefix_promotes_status() {
        let mut findings = vec![finding("UNI-014", "src/lib.rs", 18)];
        let dirs = vec![directive(
            "src/lib.rs",
            17,
            "UNI-014",
            18,
            Some("false-positive: scanner misfires for the demo stub URL"),
        )];
        let outcome = apply(&mut findings, &dirs, &validation_rules(), 100);
        assert!(outcome.synthetics.is_empty());

        let stamped = &findings[0];
        assert_eq!(stamped.status, Some(FindingStatus::FalsePositive));
        let rationale =
            &stamped.disposition.as_ref().unwrap().directive.as_ref().unwrap().rationale;
        assert!(rationale.starts_with("false-positive:"));
    }

    /// Test 3a: directive with `rationale = None` mints UNI-022 when
    /// the rule is resolved.
    #[test]
    fn missing_rationale_mints_uni_022_when_resolved() {
        let mut findings = vec![finding("UNI-014", "src/lib.rs", 18)];
        let dirs = vec![directive("src/lib.rs", 17, "UNI-014", 18, None)];
        let outcome = apply(&mut findings, &dirs, &validation_rules(), 100);

        assert_eq!(
            outcome.synthetics.len(),
            1,
            "exactly one UNI-022 expected: {:?}",
            outcome.synthetics
        );
        let synth = &outcome.synthetics[0];
        assert_eq!(synth.rule_id.as_deref(), Some(UNI_022));
        assert_eq!(synth.status, Some(FindingStatus::Open));
        let location = synth.location.as_ref().expect("synthetic carries location");
        assert_eq!(location.path, "src/lib.rs");
        assert_eq!(location.line, Some(17));
        assert!(synth.disposition.is_none());
        assert_eq!(synth.id, "FIND-0100");
        assert_eq!(outcome.next_id_counter, 101);
    }

    /// Test 3b: missing rationale is silent when UNI-022 is not in
    /// the resolved set (graceful degradation per RFC-33a §"Graceful
    /// degradation when the universal codex tree is absent"). The
    /// match-and-demote step still runs against the matched finding.
    #[test]
    fn missing_rationale_silent_when_uni_022_absent() {
        let mut findings = vec![finding("UNI-014", "src/lib.rs", 18)];
        let dirs = vec![directive("src/lib.rs", 17, "UNI-014", 18, None)];
        // Only UNI-023 is resolved; UNI-022 absent.
        let resolved = vec![rule(UNI_023, Severity::Important)];
        let outcome = apply(&mut findings, &dirs, &resolved, 100);
        assert!(
            outcome.synthetics.is_empty(),
            "no UNI-022 must be minted; UNI-023 would only fire on an orphan",
        );
        // The match still stamps the finding.
        assert_eq!(findings[0].status, Some(FindingStatus::Ignored));
    }

    /// Test 4: rationale shorter than 16 characters mints UNI-022
    /// with the same gating as test 3.
    #[test]
    fn short_rationale_mints_uni_022() {
        let mut findings = vec![finding("UNI-014", "src/lib.rs", 18)];
        let dirs = vec![directive("src/lib.rs", 17, "UNI-014", 18, Some("too short"))];
        let outcome = apply(&mut findings, &dirs, &validation_rules(), 1);
        assert_eq!(outcome.synthetics.len(), 1);
        assert_eq!(outcome.synthetics[0].rule_id.as_deref(), Some(UNI_022));

        // Empty codex: no synthetic.
        let mut findings = vec![finding("UNI-014", "src/lib.rs", 18)];
        let dirs = vec![directive("src/lib.rs", 17, "UNI-014", 18, Some("too short"))];
        let outcome = apply(&mut findings, &dirs, &[], 1);
        assert!(outcome.synthetics.is_empty());
    }

    /// Test 5: directive whose `rule_id` does not match any finding on
    /// its target line mints UNI-023 when resolved, and silently when
    /// not.
    #[test]
    fn orphan_directive_mints_uni_023() {
        let mut findings = vec![finding("UNI-014", "src/lib.rs", 18)];
        let dirs = vec![directive(
            "src/lib.rs",
            17,
            "UNI-099",
            18,
            Some("explicit unmatched rule id reference"),
        )];
        let outcome = apply(&mut findings, &dirs, &validation_rules(), 100);
        let ids: Vec<&str> =
            outcome.synthetics.iter().filter_map(|f| f.rule_id.as_deref()).collect();
        assert!(ids.contains(&UNI_023), "expected UNI-023 in synthetics; got {ids:?}");
        assert!(!ids.contains(&UNI_022), "UNI-022 must not fire for long rationale");
        // The matched finding stays Open.
        assert!(findings[0].status.is_none());

        // Graceful degradation: UNI-023 missing → no synthetic.
        let mut findings = vec![finding("UNI-014", "src/lib.rs", 18)];
        let dirs = vec![directive(
            "src/lib.rs",
            17,
            "UNI-099",
            18,
            Some("explicit unmatched rule id reference"),
        )];
        let outcome = apply(&mut findings, &dirs, &[], 100);
        assert!(outcome.synthetics.is_empty());
    }

    /// Test 6: two directives on consecutive lines composing onto the
    /// same target line — each targets a different rule and stamps
    /// its own finding.
    #[test]
    fn composed_directives_stamp_two_different_findings() {
        let mut findings =
            vec![finding("UNI-014", "src/lib.rs", 20), finding("UNI-015", "src/lib.rs", 20)];
        let dirs = vec![
            directive("src/lib.rs", 18, "UNI-014", 20, Some("first rationale long enough text")),
            directive("src/lib.rs", 19, "UNI-015", 20, Some("second rationale long enough text")),
        ];
        let outcome = apply(&mut findings, &dirs, &validation_rules(), 100);
        assert!(
            outcome.synthetics.is_empty(),
            "no synthetic expected; got {:?}",
            outcome.synthetics
        );
        assert_eq!(findings[0].status, Some(FindingStatus::Ignored));
        assert_eq!(findings[1].status, Some(FindingStatus::Ignored));
        let p0 = findings[0].disposition.as_ref().unwrap().directive.as_ref().unwrap();
        let p1 = findings[1].disposition.as_ref().unwrap().directive.as_ref().unwrap();
        assert_eq!(p0.line, 18);
        assert_eq!(p1.line, 19);
    }

    /// Test 7: a directive whose path differs from the finding's path
    /// never matches, even when the line and rule id agree. The
    /// directive surfaces as an orphan.
    #[test]
    fn directive_path_mismatch_is_orphan() {
        let mut findings = vec![finding("UNI-014", "src/file_b.rs", 18)];
        let dirs = vec![directive(
            "src/file_a.rs",
            17,
            "UNI-014",
            18,
            Some("rationale long enough to clear the floor"),
        )];
        let outcome = apply(&mut findings, &dirs, &validation_rules(), 100);
        assert!(findings[0].status.is_none(), "cross-file finding must not be stamped");
        let ids: Vec<&str> =
            outcome.synthetics.iter().filter_map(|f| f.rule_id.as_deref()).collect();
        assert_eq!(ids, vec![UNI_023]);
    }

    /// Determinism: identical inputs in shuffled order produce
    /// identical synthetic output (ordering + content).
    #[test]
    fn apply_output_order_independent() {
        let dirs_a = vec![
            directive("a.rs", 5, "UNI-099", 6, Some("rationale long enough to clear")),
            directive("b.rs", 7, "UNI-100", 8, None),
        ];
        let dirs_b = vec![dirs_a[1].clone(), dirs_a[0].clone()];

        let mut findings_a: Vec<LintFinding> = Vec::new();
        let mut findings_b: Vec<LintFinding> = Vec::new();
        let resolved = validation_rules();
        let out_a = apply(&mut findings_a, &dirs_a, &resolved, 1);
        let out_b = apply(&mut findings_b, &dirs_b, &resolved, 1);
        assert_eq!(out_a, out_b);
    }

    /// Test 8: status-aware exit helper — only `Open` (or unset)
    /// findings with `critical | important` severity block.
    #[test]
    fn blocking_findings_present_respects_status() {
        // Open + Important → blocks.
        let mut open_important = finding("UNI-014", "src/lib.rs", 1);
        open_important.severity = Severity::Important;
        open_important.status = Some(FindingStatus::Open);
        assert!(blocking_findings_present(std::slice::from_ref(&open_important)));

        // Unset status + Critical → blocks (treated as open).
        let mut unset_critical = finding("UNI-014", "src/lib.rs", 2);
        unset_critical.severity = Severity::Critical;
        unset_critical.status = None;
        assert!(blocking_findings_present(std::slice::from_ref(&unset_critical)));

        // Ignored + Critical → does NOT block.
        let mut ignored_critical = finding("UNI-014", "src/lib.rs", 3);
        ignored_critical.severity = Severity::Critical;
        ignored_critical.status = Some(FindingStatus::Ignored);
        assert!(!blocking_findings_present(std::slice::from_ref(&ignored_critical)));

        // FalsePositive + Important → does NOT block.
        let mut fp_important = finding("UNI-014", "src/lib.rs", 4);
        fp_important.severity = Severity::Important;
        fp_important.status = Some(FindingStatus::FalsePositive);
        assert!(!blocking_findings_present(std::slice::from_ref(&fp_important)));

        // Open + Suggestion → does NOT block (severity below the
        // blocking threshold).
        let mut open_suggestion = finding("UNI-014", "src/lib.rs", 5);
        open_suggestion.severity = Severity::Suggestion;
        open_suggestion.status = Some(FindingStatus::Open);
        assert!(!blocking_findings_present(std::slice::from_ref(&open_suggestion)));

        // Mixed set with one open-critical hidden among ignored ones:
        // still blocks.
        let set = vec![ignored_critical.clone(), open_important, open_suggestion];
        assert!(blocking_findings_present(&set));

        // All ignored / false-positive: no blocking.
        let safe_set = vec![ignored_critical, fp_important];
        assert!(!blocking_findings_present(&safe_set));

        // Empty: no blocking.
        assert!(!blocking_findings_present(&[]));
    }
}
