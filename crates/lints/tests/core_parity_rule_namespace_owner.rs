//! C17 parity test: prove `CORE-009` covers the `namespace-owner`
//! reserved-kind semantics for framework rule files — each rule's
//! id-namespace prefix must be authored only under the rules directory
//! that owns that namespace.
//!
//! # Equivalence mapping
//!
//! Declarative rule id `CORE-009` ≅ the namespace-ownership branch of
//! the imperative `check::rules` predicate
//! (`rules.namespace-ownership-violation`). **No imperative `Check` row
//! is retired by this card.** The C17 plan card named the
//! `crates/lints/src/framework/check/rules.rs` `BUILTIN_NAMESPACES`
//! owner-mismatch row as a strong candidate, but `namespace-owner` does
//! not subsume it cleanly:
//!
//! - `run_rules_check` is a single fused predicate that emits three
//!   distinct finding kinds (`rules.schema-violation`,
//!   `rules.namespace-ownership-violation`, `rules.duplicate-rule-id`);
//!   the ownership branch is not a separable row.
//! - The imperative ownership branch also owns the `FRAME-*`
//!   reservation, dynamic source-adapter owner discovery (walking
//!   `adapters/sources/<name>/rules/`), and the unknown-owner
//!   diagnostic — none of which a single fact-iterating evaluator can
//!   replicate.
//!
//! So per the §F5 migration cadence the kind interpreter plus seed rule
//! land against a synthetic fixture (the C14 / C15 / C16 fallback): the
//! interpreter ships, `CORE-009` is the smoke-test landing rule, and
//! the imperative predicate stays. Because no imperative deletion is in
//! flight, the fingerprint-based deduplication has nothing to merge.
//!
//! Imperative behaviour (anchored as executable code in this test
//! crate): for every rule markdown file under
//! `adapters/{shared,sources,targets}/.../rules/`, parse the `id:`
//! frontmatter, derive the `PREFIX` from a `PREFIX-NNN` id, resolve the
//! id-prefix set owned by the containing rules directory
//! (`universal → UNI`, `core → CORE`, `omnia → {OMNIA,RUST,SEC}`,
//! `contracts → IFACE`, `vectis → VECTIS`, `sources/<name> → SRC`), and
//! return the set of rule paths whose prefix is not in the owned set.
//!
//! Declarative behaviour: the framework-profile indexer extracts one
//! [`specify_lints::lint::Frontmatter`] fact per rule markdown file
//! (`crates/lints/src/lint/index/frontmatter.rs::extract`); the
//! `kind: namespace-owner` interpreter
//! (`crates/lints/src/lint/eval/namespace_owner.rs::evaluate`)
//! narrows the candidate set with the `path-pattern` hint, reads each
//! candidate's `id`, derives the same owned-prefix set from the path,
//! and emits one [`Diagnostic`] per misplaced rule carrying the
//! `(rule, rule-id, namespace, owner, allowed)` shape as structured
//! evidence.
//!
//! # Option
//!
//! Option A (functional parity) against a synthetic fixture. The test
//! stages a framework tree with five rule files: three correctly placed
//! (`CORE` under core, `OMNIA` under omnia, `SRC` under a source
//! adapter) and two misplaced (`UNI` under core, `VECTIS` under omnia).
//! Both the inline imperative reference and the declarative `CORE-009`
//! pipeline MUST agree on the set of misplaced rule paths.
//!
//! Per-finding locations are NOT compared byte-identically; functional
//! parity (which rule files were misplaced) is the contract.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use specify_diagnostics::{Diagnostic, FindingEvidence, Severity};
use specify_lints::lint::ScanProfile;
use specify_lints::lint::eval::{ToolOutput, ToolRunError, ToolRunner, evaluate};
use specify_lints::lint::index::build;
use specify_lints::rules::{DeterministicHint, HintKind, Origin, PathRoot, ResolvedRule};

const RULE_GLOB: &str = "adapters/**/rules/**/*.md";

/// `(relative path, rule id)` for every staged rule file.
const RULES: &[(&str, &str)] = &[
    ("adapters/shared/rules/core/CORE-001-aligned.md", "CORE-001"),
    ("adapters/shared/rules/core/UNI-misplaced.md", "UNI-001"),
    ("adapters/targets/omnia/rules/OMNIA-001-aligned.md", "OMNIA-001"),
    ("adapters/targets/omnia/rules/VECTIS-misplaced.md", "VECTIS-001"),
    ("adapters/sources/documentation/rules/SRC-001-aligned.md", "SRC-001"),
];

/// Stage the synthetic framework tree of rule files.
fn stage_project(project_dir: &Path) {
    for (rel, id) in RULES {
        let path = project_dir.join(rel);
        fs::create_dir_all(path.parent().expect("rule parent")).expect("create parent");
        let body = format!(
            "---\nid: {id}\ntitle: Parity Fixture\nseverity: optional\ntrigger: Namespace ownership parity fixture covering rule placement.\n---\n\n## Rule\n\nBody.\n"
        );
        fs::write(path, body).expect("write rule");
    }
}

/// Inline reference mirroring the `kind: namespace-owner` semantics so
/// the parity claim is anchored to executable code in this commit.
/// Returns the set of rule paths whose id-prefix is not owned by the
/// containing rules directory.
fn imperative_misplaced_set(project_dir: &Path) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for (rel, id) in RULES {
        drop(fs::read_to_string(project_dir.join(rel)).expect("rule readable"));
        let Some(allowed) = owned_namespaces(rel) else { continue };
        let Some(prefix) = namespace_prefix(id) else { continue };
        if !allowed.contains(prefix) {
            out.insert((*rel).to_string());
        }
    }
    out
}

fn owned_namespaces(path: &str) -> Option<BTreeSet<&'static str>> {
    if path.starts_with("adapters/shared/rules/universal/") {
        return Some(BTreeSet::from(["UNI"]));
    }
    if path.starts_with("adapters/shared/rules/core/") {
        return Some(BTreeSet::from(["CORE"]));
    }
    let targets: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::from([
        ("omnia", BTreeSet::from(["OMNIA", "RUST", "SEC"])),
        ("contracts", BTreeSet::from(["IFACE"])),
        ("vectis", BTreeSet::from(["VECTIS"])),
    ]);
    if let Some(rest) = path.strip_prefix("adapters/targets/")
        && let Some((name, tail)) = rest.split_once('/')
        && tail.starts_with("rules/")
    {
        return targets.get(name).cloned();
    }
    if let Some(rest) = path.strip_prefix("adapters/sources/")
        && let Some((_, tail)) = rest.split_once('/')
        && tail.starts_with("rules/")
    {
        return Some(BTreeSet::from(["SRC"]));
    }
    None
}

fn namespace_prefix(id: &str) -> Option<&str> {
    let (prefix, suffix) = id.split_once('-')?;
    let well_formed = !prefix.is_empty()
        && prefix.bytes().all(|b| b.is_ascii_uppercase())
        && suffix.len() == 3
        && suffix.bytes().all(|b| b.is_ascii_digit());
    well_formed.then_some(prefix)
}

fn declarative_misplaced_set(findings: &[Diagnostic]) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for finding in findings {
        let FindingEvidence::Structured { data, .. } = &finding.evidence else { continue };
        if let Some(rule) = data.get("rule").and_then(|v| v.as_str()) {
            out.insert(rule.to_string());
        }
    }
    out
}

fn make_rule(rule_id: &str, hints: Vec<DeterministicHint>) -> ResolvedRule {
    ResolvedRule {
        rule_id: rule_id.to_string(),
        title: format!("{rule_id} parity fixture"),
        severity: Severity::Important,
        trigger: format!("Trigger for {rule_id}"),
        lint_mode: None,
        applicability: None,
        deterministic_hints: if hints.is_empty() { None } else { Some(hints) },
        references: None,
        origin: Origin::Core,
        path_root: PathRoot::RulesRoot,
        path: format!("adapters/shared/rules/core/{rule_id}.md"),
        body: String::new(),
        deprecated: None,
    }
}

fn hint(kind: HintKind, value: &str) -> DeterministicHint {
    DeterministicHint {
        kind,
        value: value.to_string(),
        description: None,
    }
}

struct NoToolRunner;

impl ToolRunner for NoToolRunner {
    fn run(
        &self, _tool_name: &str, _args: &[String], _project_dir: &Path,
    ) -> Result<ToolOutput, ToolRunError> {
        Err(ToolRunError::Runtime("no tool runner wired".to_string()))
    }

    fn is_declared(&self, _tool_name: &str) -> bool {
        false
    }
}

#[test]
fn core_009_matches_namespace_owner_reference_against_rule_files() {
    let project = tempfile::tempdir().expect("tempdir");
    let project_dir = project.path();
    stage_project(project_dir);

    let imperative = imperative_misplaced_set(project_dir);
    let expected: BTreeSet<String> = [
        "adapters/shared/rules/core/UNI-misplaced.md".to_string(),
        "adapters/targets/omnia/rules/VECTIS-misplaced.md".to_string(),
    ]
    .into_iter()
    .collect();
    assert_eq!(
        imperative, expected,
        "imperative reference must flag exactly the two misplaced rule files",
    );

    let model = build(project_dir, ScanProfile::Framework, &[], &[]).expect("framework build");
    let rule = make_rule(
        "CORE-009",
        vec![
            hint(HintKind::PathPattern, RULE_GLOB),
            hint(HintKind::NamespaceOwner, "rule-namespace-matches-owner"),
        ],
    );
    let runner: &dyn ToolRunner = &NoToolRunner;
    let outcome = evaluate(
        &rule,
        rule.deterministic_hints.as_deref().unwrap_or_default(),
        &model,
        project_dir,
        runner,
        1,
    )
    .expect("declarative evaluate");

    for finding in &outcome.findings {
        assert_eq!(
            finding.rule_id.as_deref(),
            Some("CORE-009"),
            "declarative findings must carry the documented CORE-009 rule id",
        );
        let loc = finding.location.as_ref().expect("location set");
        assert!(
            Path::new(&loc.path).extension().is_some_and(|ext| ext.eq_ignore_ascii_case("md")),
            "declarative location must point at a rule markdown file: got {}",
            loc.path,
        );
    }

    let declarative = declarative_misplaced_set(&outcome.findings);
    assert_eq!(
        declarative, imperative,
        "declarative CORE-009 must flag the same rule files as the inline namespace-owner reference",
    );
}
