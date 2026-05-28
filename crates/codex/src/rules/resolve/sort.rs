//! Stable export ordering and `ResolvedCodex` assembly (CH-14).
//!
//! Implements RFC-28 §"Resolved codex export" §"Ordering". CH-12 owns
//! discovery, CH-13 owns filtering; this module sorts the survivors
//! by the closed four-tuple and lifts each [`ResolvedRuleEntry`] into
//! a wire-shaped [`ResolvedRule`] inside a [`ResolvedCodex`] envelope.
//!
//! # Sort tuple
//!
//! Per RFC-28 §"Ordering" the closed sort key is:
//!
//! 1. **non-deprecated before deprecated** — `rule.deprecated.is_some()`
//!    maps to `false < true`, putting active rules ahead of historical
//!    citations.
//! 2. **severity** — `critical < important < suggestion < optional`.
//! 3. **origin** — `target < source < shared < organization`.
//! 4. **`rule-id`** lexical.
//!
//! Both [`super::super::Severity`] and [`super::super::Origin`] are
//! declared with variants in the RFC sort order in
//! [`crate::rules`], so the derived [`Ord`] picks up the
//! RFC-mandated comparator directly — no bespoke `_sort_key` helpers
//! needed. The `severity_ordering_matches_rfc` and
//! `origin_ordering_matches_rfc` tests in `crates/codex/src/rules.rs`
//! pin that declaration order so a future refactor cannot silently
//! shift the sort.
//!
//! [`slice::sort_by`] is a stable sort, so ties on the closed four-tuple
//! preserve CH-12's lexical intra-directory ordering. That keeps
//! same-id-prefix rules from the same overlay rung in the order
//! `list_rule_files` produced them.
//!
//! # Path stability
//!
//! [`ResolvedRule::path`] is carried verbatim from
//! [`ResolvedRuleEntry::path`] (already relative to
//! [`ResolvedRule::path_root`] per CH-12 §"Compute `path` relative to
//! `root`"). The resolver writes forward-slash paths on every host,
//! so golden bytes match across Linux, macOS, and Windows.

use super::{ResolveError, ResolveInputs, ResolvedRuleEntry, filter};
use crate::rules::{CodexRule, ResolvedCodex, ResolvedRule};

/// Sort `entries` in place by the closed RFC-28 four-tuple.
///
/// See the module docs for the ordering rationale. [`slice::sort_by`]
/// is stable, so ties on the four-tuple preserve CH-12's lexical
/// intra-directory ordering.
pub fn sort_resolved(entries: &mut [ResolvedRuleEntry]) {
    entries.sort_by(|a, b| {
        let key_a = (a.rule.deprecated.is_some(), a.rule.severity, a.origin, a.rule.id.as_str());
        let key_b = (b.rule.deprecated.is_some(), b.rule.severity, b.origin, b.rule.id.as_str());
        key_a.cmp(&key_b)
    });
}

/// Compose [`super::resolve`], [`super::filter`], and [`sort_resolved`]
/// to assemble the [`ResolvedCodex`] wire envelope.
///
/// This is the top-level entry point CH-17 (the `specrun rules
/// export` CLI) will call. The returned envelope is fully ordered and
/// ready for serialisation against `resolved.schema.json`.
///
/// # Errors
///
/// Returns the same [`ResolveError`] variants as the underlying
/// [`mod@super::super::resolve`] call; sort + lift are infallible.
pub fn build_resolved_codex(inputs: &ResolveInputs<'_>) -> Result<ResolvedCodex, ResolveError> {
    let mut entries = filter(super::resolve(inputs)?, inputs);
    sort_resolved(&mut entries);
    let rules = entries.into_iter().map(entry_into_resolved_rule).collect();
    Ok(ResolvedCodex {
        version: 1,
        target_adapter: inputs.target_adapter.to_string(),
        source_adapters: inputs.source_adapters.to_vec(),
        rules,
    })
}

/// Lift a [`ResolvedRuleEntry`] into a wire-shaped [`ResolvedRule`].
///
/// Consumes the entry so `rule.body` and the other owned strings move
/// into the result without cloning. The `rule_id` wire field comes
/// from [`CodexRule::id`]; the rename is documented on
/// [`ResolvedRule::rule_id`].
fn entry_into_resolved_rule(entry: ResolvedRuleEntry) -> ResolvedRule {
    let ResolvedRuleEntry {
        rule,
        origin,
        path_root,
        path,
    } = entry;
    let CodexRule {
        id,
        title,
        severity,
        trigger,
        lint_mode,
        applicability,
        deterministic_hints,
        references,
        deprecated,
        body,
    } = rule;
    ResolvedRule {
        rule_id: id,
        title,
        severity,
        trigger,
        lint_mode,
        applicability,
        deterministic_hints,
        references,
        origin,
        path_root,
        path,
        body,
        deprecated,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use tempfile::TempDir;

    use super::*;
    use crate::rules::{Deprecated, Origin, PathRoot, Severity};

    fn rule(id: &str, severity: Severity, deprecated: bool) -> CodexRule {
        CodexRule {
            id: id.into(),
            title: format!("{id} fixture"),
            severity,
            trigger: "Synthetic CH-14 sort fixture trigger sentence long enough for schema.".into(),
            lint_mode: None,
            applicability: None,
            deterministic_hints: None,
            references: None,
            deprecated: deprecated.then(|| Deprecated {
                reason: "fixture deprecation".into(),
                replaced_by: None,
            }),
            body: format!("## Rule\n\nBody for {id}.\n"),
        }
    }

    fn entry(id: &str, severity: Severity, origin: Origin, deprecated: bool) -> ResolvedRuleEntry {
        ResolvedRuleEntry {
            rule: rule(id, severity, deprecated),
            origin,
            path_root: PathRoot::CodexRoot,
            path: format!("adapters/shared/codex/universal/{id}.md"),
        }
    }

    fn ids(entries: &[ResolvedRuleEntry]) -> Vec<&str> {
        entries.iter().map(|e| e.rule.id.as_str()).collect()
    }

    fn ids_of_rules(rules: &[ResolvedRule]) -> Vec<&str> {
        rules.iter().map(|r| r.rule_id.as_str()).collect()
    }

    /// Test 3: deprecated entries sort after non-deprecated entries
    /// regardless of other tie-breakers.
    #[test]
    fn sort_puts_non_deprecated_first() {
        let mut entries = vec![
            entry("RULE-A", Severity::Important, Origin::Shared, true),
            entry("RULE-A2", Severity::Important, Origin::Shared, false),
        ];
        sort_resolved(&mut entries);
        assert_eq!(ids(&entries), vec!["RULE-A2", "RULE-A"]);
    }

    /// Test 4: ties on (deprecated, severity, origin) resolve by
    /// lexical `rule-id`.
    #[test]
    fn sort_breaks_ties_by_rule_id() {
        let mut entries = vec![
            entry("OMNIA-002", Severity::Critical, Origin::Target, false),
            entry("OMNIA-001", Severity::Critical, Origin::Target, false),
            entry("OMNIA-003", Severity::Critical, Origin::Target, false),
        ];
        sort_resolved(&mut entries);
        assert_eq!(ids(&entries), vec!["OMNIA-001", "OMNIA-002", "OMNIA-003"]);
    }

    /// Test 5: full-tuple precedence — deprecation dominates severity
    /// dominates origin dominates id. Walks through a mix that
    /// triggers every comparator dimension.
    #[test]
    fn sort_full_tuple_precedence() {
        let mut entries = vec![
            entry("A", Severity::Critical, Origin::Target, true),
            entry("Z", Severity::Optional, Origin::Shared, false),
            entry("M", Severity::Critical, Origin::Source, false),
        ];
        sort_resolved(&mut entries);
        // Z (non-deprecated, Optional, Shared) and M (non-deprecated,
        // Critical, Source) both beat A (deprecated). M's Critical
        // beats Z's Optional.
        assert_eq!(ids(&entries), vec!["M", "Z", "A"]);
    }

    /// Helper: a minimal frontmatter + body that parses through CH-11
    /// and validates against the codex-rule schema.
    fn rule_markdown(id: &str, title: &str, severity: &str) -> String {
        format!(
            "---\nid: {id}\ntitle: {title}\nseverity: {severity}\ntrigger: Synthetic CH-14 build_resolved_codex fixture trigger sentence long enough for schema.\n---\n\n## Rule\n\nBody for {id}.\n"
        )
    }

    fn write_rule(path: &Path, id: &str, title: &str, severity: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent dir");
        }
        fs::write(path, rule_markdown(id, title, severity)).expect("write rule fixture");
    }

    fn run_build(codex_root: &Path, project_dir: &Path) -> ResolvedCodex {
        let sources: Vec<String> = Vec::new();
        let inputs = ResolveInputs {
            project_dir,
            codex_root: Some(codex_root),
            target_adapter: "omnia",
            source_adapters: &sources,
            artifact_paths: &[],
            languages: &[],
            include_deprecated: false,
            include_unmatched: false,
        };
        build_resolved_codex(&inputs).expect("build_resolved_codex succeeds")
    }

    /// Test 6: `build_resolved_codex` integration — wire envelope is
    /// versioned, target/source carry through, and rules emerge
    /// sorted per the closed four-tuple.
    #[test]
    fn build_resolved_codex_emits_versioned_envelope() {
        let codex_root = TempDir::new().expect("codex root");
        let project = TempDir::new().expect("project");
        write_rule(
            &codex_root.path().join("adapters/shared/codex/universal/uni-002.md"),
            "UNI-002",
            "Important shared",
            "important",
        );
        write_rule(
            &codex_root.path().join("adapters/shared/codex/universal/uni-001.md"),
            "UNI-001",
            "Critical shared",
            "critical",
        );
        write_rule(
            &project.path().join("adapters/targets/omnia/codex/omnia-001.md"),
            "OMNIA-001",
            "Important target",
            "important",
        );

        let resolved = run_build(codex_root.path(), project.path());

        assert_eq!(resolved.version, 1);
        assert_eq!(resolved.target_adapter, "omnia");
        assert!(resolved.source_adapters.is_empty());
        assert_eq!(resolved.rules.len(), 3);
        // UNI-001 is Critical (beats OMNIA-001 Important); OMNIA-001
        // is Important + Target (beats UNI-002 Important + Shared);
        // UNI-002 trails.
        assert_eq!(ids_of_rules(&resolved.rules), vec!["UNI-001", "OMNIA-001", "UNI-002"]);
    }

    /// Test 7: paths on the wire envelope are anchored to `path-root`
    /// and never absolute (no leading `/` on Unix, no `<drive>:` on
    /// Windows). Guards the cross-platform determinism the plan calls
    /// out.
    #[test]
    fn resolved_paths_are_anchored_and_not_absolute() {
        let codex_root = TempDir::new().expect("codex root");
        let project = TempDir::new().expect("project");
        write_rule(
            &codex_root.path().join("adapters/shared/codex/universal/uni-001.md"),
            "UNI-001",
            "Shared",
            "important",
        );
        write_rule(
            &project.path().join("adapters/targets/omnia/codex/omnia-001.md"),
            "OMNIA-001",
            "Target",
            "important",
        );

        let resolved = run_build(codex_root.path(), project.path());
        for rule in &resolved.rules {
            assert!(
                !rule.path.starts_with('/'),
                "rule {} path leaked an absolute prefix: {}",
                rule.rule_id,
                rule.path,
            );
            // Windows drive-letter guard: a `<letter>:` prefix would
            // mean the resolver failed to strip the temp dir root.
            let bytes = rule.path.as_bytes();
            let drive_letter =
                bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
            assert!(
                !drive_letter,
                "rule {} path leaked a Windows drive prefix: {}",
                rule.rule_id, rule.path,
            );
            assert!(
                !rule.path.contains('\\'),
                "rule {} path leaked a backslash separator: {}",
                rule.rule_id,
                rule.path,
            );
        }
    }

    /// Test 8: identical inputs produce byte-identical JSON across
    /// runs. Pins the stability guarantee CH-17 will rely on for
    /// golden tests.
    #[test]
    fn build_resolved_codex_is_byte_stable_across_runs() {
        let codex_root = TempDir::new().expect("codex root");
        let project = TempDir::new().expect("project");
        write_rule(
            &codex_root.path().join("adapters/shared/codex/universal/uni-001.md"),
            "UNI-001",
            "Shared",
            "critical",
        );
        write_rule(
            &codex_root.path().join("adapters/shared/codex/universal/uni-002.md"),
            "UNI-002",
            "Shared opt",
            "optional",
        );
        write_rule(
            &project.path().join("adapters/targets/omnia/codex/omnia-001.md"),
            "OMNIA-001",
            "Target",
            "important",
        );

        let first = run_build(codex_root.path(), project.path());
        let second = run_build(codex_root.path(), project.path());
        let first_json = serde_json::to_string(&first).expect("serialise first");
        let second_json = serde_json::to_string(&second).expect("serialise second");
        assert_eq!(first_json, second_json);
    }
}
