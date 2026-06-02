//! `kind: namespace-owner` evaluator.
//!
//! Asserts that each rule file's id-namespace prefix is authored only
//! under the directory that OWNS that namespace. v1 supports one
//! source discriminator — `rule-namespace-matches-owner` — which
//! reads the `id:` field of every candidate rule markdown file from
//! the [`crate::lint::Frontmatter`] facts the indexer already produced
//! (see [`crate::lint::index::frontmatter::extract`]) and flags each
//! rule whose id prefix is not owned by its containing rules
//! directory. The interpreter emits one [`specify_diagnostics::Diagnostic`]
//! per misplaced rule, with the rule file path as the finding's
//! location and the `(rule-id, namespace, owner, allowed)` shape
//! surfaced via [`specify_diagnostics::FindingEvidence::Structured`] for
//! downstream tooling.
//!
//! This is the declarative form of the hand-written namespace
//! ownership predicate in `check::rules`
//! (`rules.namespace-ownership-violation`). The owner → allowed-prefix
//! table here mirrors that predicate's `BUILTIN_NAMESPACES` static
//! entries plus the source-axis `SRC-*` rule; the imperative predicate
//! is NOT retired because it additionally owns the `FRAME-*`
//! reservation, dynamic source-owner discovery, the unknown-owner
//! diagnostic, schema validation, and duplicate-id detection — none of
//! which a single fact-iterating digest evaluator can replicate. See
//! the C17 parity test docstring for why the row stays. This rule is
//! the smoke-test landing path for the kind, firing zero findings
//! against the healthy framework tree and surfacing only on
//! misplacement.
//!
//! Candidate selection is driven by the `path-pattern` filter the
//! umbrella evaluator builds: only rule files in the supplied
//! candidate set are considered. A candidate whose path is not an
//! owned rules directory, or whose `id` is absent or does not match
//! the `PREFIX-NNN` shape, is skipped — the `check::rules` schema and
//! ownership predicates own those branches.
//!
//! Future hint values may extend the closed source set; unknown
//! discriminators are rejected as [`super::HintError::Unsupported`]
//! so authoring drift surfaces at hint-evaluation time rather than
//! silently passing.

use std::collections::BTreeSet;
use std::path::PathBuf;

use specify_diagnostics::{Diagnostic, FindingEvidence, FindingLocation};

use super::{HintError, make_finding};
use crate::lint::WorkspaceModel;
use crate::rules::{DeterministicHint, HintKind, ResolvedRule};

const SOURCE_RULE_NAMESPACE_MATCHES_OWNER: &str = "rule-namespace-matches-owner";

/// Owner → allowed id-prefix set, kept in sync with the static
/// entries of `crates/standards/src/framework/check/rules.rs` `BUILTIN_NAMESPACES`.
/// The source-axis `SRC-*` rule is derived from the path shape rather
/// than enumerated here so source-adapter names are never hardcoded.
const TARGET_OWNERS: &[(&str, &[&str])] =
    &[("omnia", &["OMNIA", "RUST", "SEC"]), ("contracts", &["IFACE"]), ("vectis", &["VECTIS"])];

/// Token placeholder for a candidate whose `id` field is missing or
/// not a string. Distinct from any real prefix so it never matches.
const ABSENT_ID_TOKEN: &str = "(absent)";

pub(crate) fn evaluate(
    rule: &ResolvedRule, hint: &DeterministicHint, candidates: &[PathBuf], model: &WorkspaceModel,
    next_id: &mut u64,
) -> Result<Vec<Diagnostic>, HintError> {
    let source = hint.value.trim();
    if source != SOURCE_RULE_NAMESPACE_MATCHES_OWNER {
        return Err(HintError::Unsupported {
            rule_id: rule.rule_id.clone(),
            kind: HintKind::NamespaceOwner,
            reason: "only `rule-namespace-matches-owner` is supported in v1",
        });
    }

    let candidate_set: BTreeSet<String> =
        candidates.iter().map(|p| p.to_string_lossy().into_owned()).collect();

    let mut out: Vec<Diagnostic> = Vec::new();
    for frontmatter in &model.frontmatter {
        if !candidate_set.contains(&frontmatter.path) {
            continue;
        }
        // A candidate that is not an owned rules directory is left to
        // the imperative predicate's unknown-owner branch.
        let Some(allowed) = owned_namespaces(&frontmatter.path) else {
            continue;
        };
        let id = frontmatter.fields.get("id").and_then(|v| v.as_str());
        // A missing or malformed id is the schema predicate's concern;
        // only a well-formed `PREFIX-NNN` id participates here.
        let Some(namespace) = id.and_then(namespace_prefix) else {
            continue;
        };
        if allowed.contains(&namespace) {
            continue;
        }

        let id_token = id.unwrap_or(ABSENT_ID_TOKEN);
        let allowed_sorted: Vec<String> = {
            let mut values: Vec<String> = allowed.iter().map(|s| (*s).to_string()).collect();
            values.sort_unstable();
            values
        };
        let owner = owner_label(&frontmatter.path).unwrap_or("(unknown)");
        let location = FindingLocation {
            path: frontmatter.path.clone(),
            line: Some(1),
            column: None,
            end_line: None,
            end_column: None,
        };
        let evidence = FindingEvidence::Structured {
            summary: format!(
                "rule '{}' namespace '{}-*' is not owned by rules directory owner '{}' (allowed: {})",
                id_token,
                namespace,
                owner,
                allowed_sorted.join(", "),
            ),
            data: serde_json::json!({
                "rule": frontmatter.path,
                "rule-id": id_token,
                "namespace": namespace,
                "owner": owner,
                "allowed": allowed_sorted,
            }),
            locations: None,
        };
        let title = format!(
            "{}: rule '{}' namespace '{}-*' not owned by '{}'",
            rule.title, id_token, namespace, owner,
        );
        let finding = make_finding(rule, *next_id, title, Some(location), evidence);
        *next_id += 1;
        out.push(finding);
    }
    Ok(out)
}

/// Resolve the id-prefix set owned by the rules directory containing
/// `path`, or `None` when `path` is not an owned rules directory.
fn owned_namespaces(path: &str) -> Option<BTreeSet<&'static str>> {
    if path.starts_with("adapters/shared/rules/universal/") {
        return Some(BTreeSet::from(["UNI"]));
    }
    if path.starts_with("adapters/shared/rules/core/") {
        return Some(BTreeSet::from(["CORE"]));
    }
    if let Some(name) = target_owner(path) {
        return TARGET_OWNERS
            .iter()
            .find(|(owner, _)| *owner == name)
            .map(|(_, prefixes)| prefixes.iter().copied().collect());
    }
    if is_source_rules_path(path) {
        return Some(BTreeSet::from(["SRC"]));
    }
    None
}

/// Human-readable owner label for diagnostics (`universal`, `core`,
/// the target-adapter name, or the source-adapter name).
fn owner_label(path: &str) -> Option<&str> {
    if path.starts_with("adapters/shared/rules/universal/") {
        return Some("universal");
    }
    if path.starts_with("adapters/shared/rules/core/") {
        return Some("core");
    }
    if let Some(name) = target_owner(path) {
        return Some(name);
    }
    source_owner(path)
}

/// Return the target-adapter name when `path` is
/// `adapters/targets/<name>/rules/…`.
fn target_owner(path: &str) -> Option<&str> {
    let rest = path.strip_prefix("adapters/targets/")?;
    let (name, tail) = rest.split_once('/')?;
    tail.strip_prefix("rules/").map(|_| name)
}

/// Return the source-adapter name when `path` is
/// `adapters/sources/<name>/rules/…`.
fn source_owner(path: &str) -> Option<&str> {
    let rest = path.strip_prefix("adapters/sources/")?;
    let (name, tail) = rest.split_once('/')?;
    tail.strip_prefix("rules/").map(|_| name)
}

fn is_source_rules_path(path: &str) -> bool {
    source_owner(path).is_some()
}

/// Extract the `PREFIX` from a `PREFIX-NNN` rule id (uppercase ASCII
/// letters, hyphen, three digits). Returns `None` for any other shape
/// so malformed ids are left to the schema predicate.
fn namespace_prefix(id: &str) -> Option<&str> {
    let (prefix, suffix) = id.split_once('-')?;
    let well_formed = !prefix.is_empty()
        && prefix.bytes().all(|b| b.is_ascii_uppercase())
        && suffix.len() == 3
        && suffix.bytes().all(|b| b.is_ascii_digit());
    well_formed.then_some(prefix)
}

#[cfg(test)]
mod tests;
