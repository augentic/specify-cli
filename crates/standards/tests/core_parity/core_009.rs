//! `CORE-009` ≅ the `namespace-owner` reserved-kind semantics: each rule's
//! id-namespace prefix must be authored only under the rules directory that
//! owns that namespace. No imperative `Check` row is retired.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use specify_diagnostics::{Diagnostic, FindingEvidence};
use specify_standards::lint::ScanProfile;
use specify_standards::lint::eval::{ToolRunner, evaluate};
use specify_standards::lint::index::build;
use specify_standards::rules::HintKind;

use crate::eval_support::{NoToolRunner, hint, make_rule};

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

/// Inline reference mirroring `kind: namespace-owner`; returns the set of
/// rule paths whose id-prefix is not owned by the containing rules directory.
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

#[test]
fn matches_namespace_owner() {
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
        rule.rule_hints.as_deref().unwrap_or_default(),
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
