//! Applicability and deprecation filters (CH-13).
//!
//! Implements RFC-28 §"Applicability" / §"Path glob semantics" /
//! §"Overlay precedence" / §"Resolved Decisions". The CH-12 resolver
//! discovers every rule visible to the export envelope; this module
//! narrows that pool down to the rules that actually apply to the
//! caller's slice context, using the inclusive AND-across-dimensions
//! semantics the RFC mandates.
//!
//! # Precedence
//!
//! Two filters run in a fixed order:
//!
//! 1. **Deprecation** — entries whose [`CodexRule::deprecated`] is
//!    populated are dropped unless [`ResolveInputs::include_deprecated`]
//!    is set. Deprecation runs **first** so an `--include-deprecated`
//!    rule still walks through applicability normally (RFC-28
//!    §"Overlay precedence": "Export includes every rule that passes
//!    applicability filtering" — `include_deprecated` does not bypass
//!    applicability).
//! 2. **Applicability** — a rule with no `applicability` block always
//!    passes ("A rule with no `applicability` block applies wherever
//!    its root applies"). A rule with an `applicability` block must
//!    pass **every populated dimension** (AND semantics).
//!
//! # Populated dimension + missing caller input
//!
//! RFC-28 §"Applicability" closes the awkward edge case: if the rule
//! populates a dimension but the caller supplied no matching input, the
//! rule is **excluded** unless [`ResolveInputs::include_unmatched`] is
//! set. This module applies that rule per dimension.
//!
//! # Per-dimension matching
//!
//! - **`adapters`** — caller input is always populated
//!   ([`ResolveInputs::target_adapter`] is required), so the
//!   missing-input branch is unreachable. The rule matches when its
//!   adapter list contains either the target adapter or any bound
//!   source adapter. The rule's adapter ref may carry an optional
//!   `@v<major>` suffix; for v1 this module strips the suffix before
//!   comparison and matches by bare name.
//! - **`languages`** — case-sensitive bare-name match against
//!   [`ResolveInputs::languages`]. Missing caller input (empty slice)
//!   excludes the rule unless `include_unmatched`.
//! - **`artifacts`** — v1 has no `--artifact-kind` input on
//!   [`ResolveInputs`]. Per the RFC's closed
//!   populated-dimension-without-caller-input rule, any rule that
//!   populates `applicability.artifacts` is **excluded unless
//!   `include_unmatched` is set**. This is an honest reflection of v1
//!   capability; a future RFC may add an explicit `--artifact-kind`
//!   input, after which `artifact_dimension_matches` would consult it
//!   the same way `language_dimension_matches` consults `languages`.
//! - **`paths`** — patterns interpreted by the `glob` crate
//!   (`glob = "0.3"`). Per RFC-28 §"Path glob semantics
//!   (`applicability.paths`)": `*` matches a single path segment
//!   without crossing `/`, `**` matches across segments, and `/` is the
//!   only separator. Matching is case-sensitive. Caller paths are
//!   normalised to forward-slash separators on Windows before being
//!   tested. Missing caller input (empty [`ResolveInputs::artifact_paths`])
//!   excludes the rule unless `include_unmatched`.
//!
//! Malformed glob patterns are treated as **non-matching** rather than
//! aborting the resolver. RFC-28 expects `specdev check` to catch
//! authoring bugs in patterns; surfacing a hard error here over a
//! single first-party typo would be more disruptive than dropping the
//! rule from the export. A future authoring-check could elevate this to
//! a hard error.
//!
//! # Composition
//!
//! Call [`super::resolve`] first, then pass its `Vec<ResolvedRuleEntry>`
//! to [`filter`] — e.g. `filter(super::resolve(inputs)?, inputs)`.
//! [`build_resolved_codex`] in the sibling `sort` module is the
//! conventional export entry point.

use std::path::{Path, PathBuf};

use glob::{MatchOptions, Pattern};

use super::{ResolveInputs, ResolvedRuleEntry};
use crate::rules::CodexRule;

/// RFC-28 §"Path glob semantics (`applicability.paths`)": case-sensitive,
/// `/` is the only separator, leading dots match literally.
const PATH_MATCH_OPTIONS: MatchOptions = MatchOptions {
    case_sensitive: true,
    require_literal_separator: true,
    require_literal_leading_dot: false,
};

/// Apply deprecation + applicability filters to a CH-12 result.
///
/// Deprecation runs first; applicability runs against the survivors.
/// See the module docs for the closed precedence rules and per-dimension
/// matching semantics.
#[must_use]
pub fn filter(
    entries: Vec<ResolvedRuleEntry>, inputs: &ResolveInputs<'_>,
) -> Vec<ResolvedRuleEntry> {
    entries
        .into_iter()
        .filter(|entry| keeps_deprecated(&entry.rule, inputs.include_deprecated))
        .filter(|entry| applicability_matches(&entry.rule, inputs))
        .collect()
}

/// `true` when the rule survives the deprecation filter.
const fn keeps_deprecated(rule: &CodexRule, include_deprecated: bool) -> bool {
    rule.deprecated.is_none() || include_deprecated
}

/// `true` when every populated applicability dimension matches.
///
/// A rule with no [`Applicability`] block always passes per RFC-28.
fn applicability_matches(rule: &CodexRule, inputs: &ResolveInputs<'_>) -> bool {
    let Some(applicability) = rule.applicability.as_ref() else {
        return true;
    };
    adapter_dimension_matches(applicability.adapters.as_deref(), inputs)
        && language_dimension_matches(
            applicability.languages.as_deref(),
            inputs.languages,
            inputs.include_unmatched,
        )
        && artifact_dimension_matches(applicability.artifacts.as_deref(), inputs.include_unmatched)
        && paths_dimension_matches(
            applicability.paths.as_deref(),
            inputs.artifact_paths,
            inputs.include_unmatched,
        )
}

/// Adapter dimension match.
///
/// Returns `true` when the rule does not constrain adapters, or when
/// the rule's adapter list contains the target adapter or any bound
/// source adapter (after stripping the optional `@v<major>` suffix on
/// the rule side).
///
/// The caller's `target_adapter` is always populated on
/// [`ResolveInputs`], so the populated-dimension-without-caller-input
/// branch is unreachable here.
fn adapter_dimension_matches(rule_adapters: Option<&[String]>, inputs: &ResolveInputs<'_>) -> bool {
    let Some(rule_adapters) = rule_adapters else {
        return true;
    };
    rule_adapters.iter().any(|raw| {
        let bare = strip_version_suffix(raw);
        bare == inputs.target_adapter
            || inputs.source_adapters.iter().any(|src| src.as_str() == bare)
    })
}

/// Language dimension match.
///
/// Returns `true` when the rule does not constrain languages. When the
/// rule populates languages but the caller supplied none, the rule is
/// excluded unless `include_unmatched`. Otherwise the rule matches when
/// any caller language appears in the rule's list.
fn language_dimension_matches(
    rule_languages: Option<&[String]>, caller_languages: &[String], include_unmatched: bool,
) -> bool {
    let Some(rule_languages) = rule_languages else {
        return true;
    };
    if caller_languages.is_empty() {
        return include_unmatched;
    }
    caller_languages.iter().any(|lang| rule_languages.iter().any(|r| r == lang))
}

/// Artifact dimension match.
///
/// v1 [`ResolveInputs`] has no `--artifact-kind` input, so any rule
/// that populates `applicability.artifacts` lacks caller input by
/// definition. Per RFC-28 §"Applicability" the populated-without-input
/// case excludes the rule unless `include_unmatched` is set. See the
/// module docs.
const fn artifact_dimension_matches(
    rule_artifacts: Option<&[String]>, include_unmatched: bool,
) -> bool {
    if rule_artifacts.is_none() {
        return true;
    }
    include_unmatched
}

/// Path-globs dimension match.
///
/// Returns `true` when the rule does not constrain paths. When the
/// rule populates paths but the caller supplied none, the rule is
/// excluded unless `include_unmatched`. Otherwise the rule matches
/// when any caller path matches any compiled rule pattern via
/// [`Pattern::matches_path_with`]. Patterns that fail to compile are
/// treated as non-matching — see the module docs.
fn paths_dimension_matches(
    rule_paths: Option<&[String]>, caller_paths: &[PathBuf], include_unmatched: bool,
) -> bool {
    let Some(rule_paths) = rule_paths else {
        return true;
    };
    if caller_paths.is_empty() {
        return include_unmatched;
    }
    let patterns: Vec<Pattern> =
        rule_paths.iter().filter_map(|pat| Pattern::new(pat).ok()).collect();
    if patterns.is_empty() {
        return false;
    }
    caller_paths
        .iter()
        .map(|p| normalise_path(p))
        .any(|candidate| patterns.iter().any(|p| p.matches_with(&candidate, PATH_MATCH_OPTIONS)))
}

/// Strip the optional `@v<major>` suffix from an adapter reference so
/// v1 matching compares bare names. `"omnia@v1"` becomes `"omnia"`;
/// `"omnia"` is returned unchanged.
fn strip_version_suffix(adapter_ref: &str) -> &str {
    adapter_ref.split_once('@').map_or(adapter_ref, |(name, _)| name)
}

/// Normalise a caller path to a forward-slash string, dropping any
/// leading `./`. Matches RFC-28 §"Path glob semantics" which fixes
/// `/` as the only separator regardless of host OS.
fn normalise_path(path: &Path) -> String {
    let displayed = path.to_string_lossy();
    let forward = if cfg!(windows) { displayed.replace('\\', "/") } else { displayed.into_owned() };
    forward.strip_prefix("./").map_or_else(|| forward.clone(), str::to_string)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::rules::{Applicability, Deprecated, Origin, PathRoot, Severity};

    fn make_rule(
        id: &str, applicability: Option<Applicability>, deprecated: Option<Deprecated>,
    ) -> CodexRule {
        CodexRule {
            id: id.into(),
            title: format!("{id} fixture"),
            severity: Severity::Important,
            trigger: "Synthetic CH-13 filter fixture trigger sentence long enough for schema."
                .into(),
            lint_mode: None,
            applicability,
            deterministic_hints: None,
            references: None,
            deprecated,
            body: format!("## Rule\n\nBody for {id}.\n"),
        }
    }

    fn make_entry(
        id: &str, applicability: Option<Applicability>, deprecated: Option<Deprecated>,
    ) -> ResolvedRuleEntry {
        ResolvedRuleEntry {
            rule: make_rule(id, applicability, deprecated),
            origin: Origin::Shared,
            path_root: PathRoot::CodexRoot,
            path: format!("adapters/shared/codex/universal/{id}.md"),
        }
    }

    fn applicability_with(
        adapters: Option<Vec<&str>>, languages: Option<Vec<&str>>, artifacts: Option<Vec<&str>>,
        paths: Option<Vec<&str>>,
    ) -> Applicability {
        Applicability {
            adapters: adapters.map(|v| v.into_iter().map(String::from).collect()),
            languages: languages.map(|v| v.into_iter().map(String::from).collect()),
            artifacts: artifacts.map(|v| v.into_iter().map(String::from).collect()),
            paths: paths.map(|v| v.into_iter().map(String::from).collect()),
        }
    }

    fn deprecation_meta() -> Deprecated {
        Deprecated {
            reason: "superseded by SEC-001".into(),
            replaced_by: Some("SEC-001".into()),
        }
    }

    fn make_inputs<'a>(
        target_adapter: &'a str, source_adapters: &'a [String], artifact_paths: &'a [PathBuf],
        languages: &'a [String], include_deprecated: bool, include_unmatched: bool,
    ) -> ResolveInputs<'a> {
        ResolveInputs {
            project_dir: Path::new("/tmp/filter-tests"),
            codex_root: None,
            target_adapter,
            source_adapters,
            artifact_paths,
            languages,
            include_deprecated,
            include_unmatched,
        }
    }

    /// Test 1: a rule with no applicability block survives any inputs.
    #[test]
    fn no_applicability_passes_through() {
        let entry = make_entry("UNI-001", None, None);
        let inputs = make_inputs("omnia", &[], &[], &[], false, false);
        let out = filter(vec![entry], &inputs);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].rule.id, "UNI-001");
    }

    /// Test 2: `adapters` matches via the caller's target adapter.
    #[test]
    fn adapters_match_target() {
        let entry = make_entry(
            "OMNIA-001",
            Some(applicability_with(Some(vec!["omnia"]), None, None, None)),
            None,
        );
        let inputs = make_inputs("omnia", &[], &[], &[], false, false);
        let out = filter(vec![entry], &inputs);
        assert_eq!(out.len(), 1);
    }

    /// Test 3: `adapters` matches via a bound source adapter.
    #[test]
    fn adapters_match_source() {
        let entry = make_entry(
            "SRC-001",
            Some(applicability_with(Some(vec!["code-typescript"]), None, None, None)),
            None,
        );
        let sources = vec!["code-typescript".to_string()];
        let inputs = make_inputs("omnia", &sources, &[], &[], false, false);
        let out = filter(vec![entry], &inputs);
        assert_eq!(out.len(), 1);
    }

    /// Test 4: `adapters` populated but neither target nor any source
    /// matches — rule filtered out. Caller input is always present for
    /// the adapter dimension, so no `include_unmatched` interaction.
    #[test]
    fn adapters_no_match_is_filtered() {
        let entry = make_entry(
            "VEC-001",
            Some(applicability_with(Some(vec!["vectis"]), None, None, None)),
            None,
        );
        let inputs = make_inputs("omnia", &[], &[], &[], false, false);
        let out = filter(vec![entry], &inputs);
        assert!(out.is_empty());
    }

    /// Test 5: `omnia@v1` on the rule side matches a bare `omnia` on
    /// the caller side — v1 strips the `@v<major>` suffix.
    #[test]
    fn adapter_version_suffix_is_stripped() {
        let entry = make_entry(
            "OMNIA-002",
            Some(applicability_with(Some(vec!["omnia@v1"]), None, None, None)),
            None,
        );
        let inputs = make_inputs("omnia", &[], &[], &[], false, false);
        let out = filter(vec![entry], &inputs);
        assert_eq!(out.len(), 1);
    }

    /// Test 6: `languages` populated, caller supplies a matching token.
    #[test]
    fn languages_match() {
        let entry = make_entry(
            "LANG-001",
            Some(applicability_with(None, Some(vec!["rust"]), None, None)),
            None,
        );
        let languages = vec!["rust".to_string()];
        let inputs = make_inputs("omnia", &[], &[], &languages, false, false);
        let out = filter(vec![entry], &inputs);
        assert_eq!(out.len(), 1);
    }

    /// Test 7: `languages` populated, caller supplies a mismatching
    /// token — rule filtered.
    #[test]
    fn languages_no_match_is_filtered() {
        let entry = make_entry(
            "LANG-002",
            Some(applicability_with(None, Some(vec!["rust"]), None, None)),
            None,
        );
        let languages = vec!["typescript".to_string()];
        let inputs = make_inputs("omnia", &[], &[], &languages, false, false);
        let out = filter(vec![entry], &inputs);
        assert!(out.is_empty());
    }

    /// Test 8: `languages` populated, caller supplies none, and
    /// `include_unmatched` is off — rule filtered.
    #[test]
    fn languages_caller_absent_excluded_by_default() {
        let entry = make_entry(
            "LANG-003",
            Some(applicability_with(None, Some(vec!["rust"]), None, None)),
            None,
        );
        let inputs = make_inputs("omnia", &[], &[], &[], false, false);
        let out = filter(vec![entry], &inputs);
        assert!(out.is_empty());
    }

    /// Test 9: `languages` populated, caller supplies none, and
    /// `include_unmatched` is on — rule passes.
    #[test]
    fn languages_caller_absent_passes_with_include_unmatched() {
        let entry = make_entry(
            "LANG-004",
            Some(applicability_with(None, Some(vec!["rust"]), None, None)),
            None,
        );
        let inputs = make_inputs("omnia", &[], &[], &[], false, true);
        let out = filter(vec![entry], &inputs);
        assert_eq!(out.len(), 1);
    }

    /// Test 10: `artifacts` populated — excluded by default because v1
    /// has no `--artifact-kind` input.
    #[test]
    fn artifacts_populated_excluded_by_default() {
        let entry = make_entry(
            "ART-001",
            Some(applicability_with(None, None, Some(vec!["code"]), None)),
            None,
        );
        let inputs = make_inputs("omnia", &[], &[], &[], false, false);
        let out = filter(vec![entry], &inputs);
        assert!(out.is_empty());
    }

    /// Test 11: `artifacts` populated + `include_unmatched` — rule
    /// passes.
    #[test]
    fn artifacts_populated_passes_with_include_unmatched() {
        let entry = make_entry(
            "ART-002",
            Some(applicability_with(None, None, Some(vec!["code"]), None)),
            None,
        );
        let inputs = make_inputs("omnia", &[], &[], &[], false, true);
        let out = filter(vec![entry], &inputs);
        assert_eq!(out.len(), 1);
    }

    /// Test 12: `paths` matches via the `**` glob across path
    /// segments.
    #[test]
    fn paths_match_double_star_segment() {
        let entry = make_entry(
            "PATH-001",
            Some(applicability_with(None, None, None, Some(vec!["crates/**/src/**/*.rs"]))),
            None,
        );
        let paths = vec![PathBuf::from("crates/billing/src/lib.rs")];
        let inputs = make_inputs("omnia", &[], &paths, &[], false, false);
        let out = filter(vec![entry], &inputs);
        assert_eq!(out.len(), 1);
    }

    /// Test 13: `paths` populated, no caller path matches — rule
    /// filtered.
    #[test]
    fn paths_no_match_is_filtered() {
        let entry = make_entry(
            "PATH-002",
            Some(applicability_with(None, None, None, Some(vec!["crates/**/src/**/*.rs"]))),
            None,
        );
        let paths = vec![PathBuf::from("README.md")];
        let inputs = make_inputs("omnia", &[], &paths, &[], false, false);
        let out = filter(vec![entry], &inputs);
        assert!(out.is_empty());
    }

    /// Test 14: `paths` populated, caller supplies no paths, and
    /// `include_unmatched` is off — rule filtered. The
    /// `include_unmatched` branch is exercised by the language test.
    #[test]
    fn paths_caller_absent_excluded_by_default() {
        let entry = make_entry(
            "PATH-003",
            Some(applicability_with(None, None, None, Some(vec!["**/*.rs"]))),
            None,
        );
        let inputs = make_inputs("omnia", &[], &[], &[], false, false);
        let out = filter(vec![entry.clone()], &inputs);
        assert!(out.is_empty());

        let inputs = make_inputs("omnia", &[], &[], &[], false, true);
        let out = filter(vec![entry], &inputs);
        assert_eq!(out.len(), 1);
    }

    /// Test 15: a single `*` segment does not cross `/`. The same
    /// pattern matches `src/lib.rs` but not `src/nested/lib.rs`.
    #[test]
    fn paths_single_star_does_not_cross_separator() {
        let entry = make_entry(
            "PATH-004",
            Some(applicability_with(None, None, None, Some(vec!["src/*.rs"]))),
            None,
        );

        let matching = vec![PathBuf::from("src/lib.rs")];
        let inputs = make_inputs("omnia", &[], &matching, &[], false, false);
        assert_eq!(filter(vec![entry.clone()], &inputs).len(), 1);

        let nested = vec![PathBuf::from("src/nested/lib.rs")];
        let inputs = make_inputs("omnia", &[], &nested, &[], false, false);
        assert!(filter(vec![entry], &inputs).is_empty());
    }

    /// Test 16: AND across dimensions — both `adapters` and
    /// `languages` must match. Adapter-only match still filters the
    /// rule when languages disagree.
    #[test]
    fn and_across_dimensions() {
        let entry = make_entry(
            "MULTI-001",
            Some(applicability_with(Some(vec!["omnia"]), Some(vec!["rust"]), None, None)),
            None,
        );

        let rust = vec!["rust".to_string()];
        let inputs = make_inputs("omnia", &[], &[], &rust, false, false);
        assert_eq!(filter(vec![entry.clone()], &inputs).len(), 1);

        let ts = vec!["typescript".to_string()];
        let inputs = make_inputs("omnia", &[], &[], &ts, false, false);
        assert!(filter(vec![entry], &inputs).is_empty());
    }

    /// Test 17: a deprecated rule is filtered when
    /// `include_deprecated` is off.
    #[test]
    fn deprecated_filtered_by_default() {
        let entry = make_entry("DEP-001", None, Some(deprecation_meta()));
        let inputs = make_inputs("omnia", &[], &[], &[], false, false);
        assert!(filter(vec![entry], &inputs).is_empty());
    }

    /// Test 18: a deprecated rule survives when `include_deprecated`
    /// is on AND its applicability (here `None`) passes.
    #[test]
    fn deprecated_passes_when_flag_set() {
        let entry = make_entry("DEP-002", None, Some(deprecation_meta()));
        let inputs = make_inputs("omnia", &[], &[], &[], true, false);
        let out = filter(vec![entry], &inputs);
        assert_eq!(out.len(), 1);
        assert!(out[0].rule.deprecated.is_some());
    }

    /// Test 19: deprecation runs before applicability. A deprecated
    /// rule whose applicability also wouldn't match is filtered out
    /// silently — not via a partial-evaluation bypass.
    #[test]
    fn deprecation_runs_before_applicability() {
        let entry = make_entry(
            "DEP-003",
            Some(applicability_with(Some(vec!["vectis"]), None, None, None)),
            Some(deprecation_meta()),
        );
        let inputs = make_inputs("omnia", &[], &[], &[], false, false);
        assert!(filter(vec![entry.clone()], &inputs).is_empty());

        // With include_deprecated on, applicability still rejects the
        // rule because the adapter list does not match.
        let inputs = make_inputs("omnia", &[], &[], &[], true, false);
        assert!(filter(vec![entry], &inputs).is_empty());
    }

    /// Test 20: a malformed glob pattern in a rule must not panic;
    /// the rule is excluded because the pattern cannot match anything.
    #[test]
    fn malformed_glob_pattern_is_safe() {
        let entry = make_entry(
            "PATH-BAD",
            Some(applicability_with(None, None, None, Some(vec!["[broken"]))),
            None,
        );
        let paths = vec![PathBuf::from("src/lib.rs")];
        let inputs = make_inputs("omnia", &[], &paths, &[], false, false);
        let out = filter(vec![entry], &inputs);
        assert!(out.is_empty());
    }
}
