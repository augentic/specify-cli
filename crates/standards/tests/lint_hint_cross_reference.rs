//! Integration test for the `cross-reference` hint evaluator.
//!
//! Exercises the generic source/target set-difference join: the
//! `adapter-dir` source family (one fact per immediate child directory
//! under `adapters/{sources,targets}`) joined against the
//! `adapter-manifest` target on the manifest's containing directory. An
//! adapter directory with no resolvable `adapter.yaml` is flagged; a
//! directory with one passes. The source / target selectors are policy
//! supplied by the rule's `value` / `config`, never a `const`
//! discriminator in the engine arm, and the test cites no specify rule
//! id.

mod eval_support;

use std::fs;
use std::path::Path;

use eval_support::{NoToolRunner, hint_with_config, make_rule};
use serde_json::json;
use specify_diagnostics::FindingEvidence;
use specify_standards::lint::ScanProfile;
use specify_standards::lint::eval::{ToolRunner, evaluate};
use specify_standards::lint::index::build;
use specify_standards::rules::{HintKind, RuleHint};

/// Create an adapter directory under `adapters/<axis>/<name>`, with an
/// `adapter.yaml` manifest when `manifest` is `Some`.
fn write_adapter_dir(project: &Path, axis: &str, name: &str, manifest: Option<&str>) {
    let dir = project.join(format!("adapters/{axis}/{name}"));
    fs::create_dir_all(&dir).expect("adapter dir");
    if let Some(body) = manifest {
        fs::write(dir.join("adapter.yaml"), body).expect("write manifest");
    }
}

fn orphan_dirs(project: &Path, hints: Vec<RuleHint>) -> Vec<String> {
    let model = build(project, ScanProfile::Framework, &[], &[]).expect("framework build");
    let rule = make_rule("UNI-962", hints);
    let runner: &dyn ToolRunner = &NoToolRunner;
    let outcome =
        evaluate(&rule, rule.rule_hints.as_deref().unwrap_or_default(), &model, project, runner, 1)
            .expect("evaluate");
    let mut paths: Vec<String> = outcome
        .findings
        .iter()
        .filter_map(|f| match &f.evidence {
            FindingEvidence::Structured { data, .. } => {
                data.get("path").and_then(|v| v.as_str()).map(str::to_string)
            }
            _ => None,
        })
        .collect();
    paths.sort();
    paths
}

fn cross_reference_hint() -> RuleHint {
    hint_with_config(
        HintKind::CrossReference,
        "adapter-dir",
        Some(json!({ "target": "adapter-manifest" })),
    )
}

/// `value: expected-set` value-equality hint joining a rule-declared
/// `{ key, value }` table against the `adapter-tool` target family.
fn expected_set_hint(entries: &serde_json::Value) -> RuleHint {
    hint_with_config(
        HintKind::CrossReference,
        "expected-set",
        Some(json!({ "target": "adapter-tool", "entries": entries.clone() })),
    )
}

#[test]
fn value_eq_flags_version_mismatch() {
    let tmp = tempfile::tempdir().expect("tmp");
    write_adapter_dir(
        tmp.path(),
        "targets",
        "vectis",
        Some("name: vectis\ntools:\n  - name: vectis\n    version: \"0.1.0\"\n"),
    );

    let flagged = orphan_dirs(
        tmp.path(),
        vec![expected_set_hint(&json!([{ "key": "vectis/vectis", "value": "0.4.0" }]))],
    );
    assert_eq!(
        flagged,
        vec!["adapters/targets/vectis/adapter.yaml".to_string()],
        "a declared tool with the wrong version is flagged at its manifest",
    );
}

#[test]
fn value_eq_flags_missing_tool() {
    // Manifest exists but does not declare the pinned tool.
    let tmp = tempfile::tempdir().expect("tmp");
    write_adapter_dir(tmp.path(), "targets", "vectis", Some("name: vectis\n"));

    let flagged = orphan_dirs(
        tmp.path(),
        vec![expected_set_hint(&json!([{ "key": "vectis/vectis", "value": "0.4.0" }]))],
    );
    assert_eq!(
        flagged,
        vec!["adapters/targets/vectis/adapter.yaml".to_string()],
        "a pinned tool absent from an existing manifest is flagged",
    );
}

#[test]
fn value_eq_passes_on_exact_match() {
    let tmp = tempfile::tempdir().expect("tmp");
    write_adapter_dir(
        tmp.path(),
        "targets",
        "vectis",
        Some("name: vectis\ntools:\n  - name: vectis\n    version: \"0.4.0\"\n"),
    );

    let flagged = orphan_dirs(
        tmp.path(),
        vec![expected_set_hint(&json!([{ "key": "vectis/vectis", "value": "0.4.0" }]))],
    );
    assert!(flagged.is_empty(), "an exact name+version match passes: {flagged:?}");
}

#[test]
fn value_eq_skips_entry_when_scope_absent() {
    let tmp = tempfile::tempdir().expect("tmp");
    write_adapter_dir(tmp.path(), "targets", "vectis", Some("name: vectis\n"));

    // `contracts` has no adapter directory at all; the join must skip its
    // entry rather than fabricate a finding for an absent group.
    let flagged = orphan_dirs(
        tmp.path(),
        vec![expected_set_hint(&json!([{ "key": "contracts/contract", "value": "0.3.0" }]))],
    );
    assert!(flagged.is_empty(), "entries whose scope has no manifest are skipped: {flagged:?}");
}

#[test]
fn flags_dir_without_manifest() {
    let tmp = tempfile::tempdir().expect("tmp");
    write_adapter_dir(tmp.path(), "targets", "omnia", Some("name: omnia\n"));
    write_adapter_dir(tmp.path(), "targets", "orphan", None);
    write_adapter_dir(tmp.path(), "sources", "intent", Some("name: intent\n"));

    let flagged = orphan_dirs(tmp.path(), vec![cross_reference_hint()]);
    assert_eq!(
        flagged,
        vec!["adapters/targets/orphan".to_string()],
        "only the directory with no resolvable manifest is flagged",
    );
}

#[test]
fn all_dirs_with_manifest_pass() {
    let tmp = tempfile::tempdir().expect("tmp");
    write_adapter_dir(tmp.path(), "targets", "omnia", Some("name: omnia\n"));
    write_adapter_dir(tmp.path(), "sources", "intent", Some("name: intent\n"));

    let flagged = orphan_dirs(tmp.path(), vec![cross_reference_hint()]);
    assert!(flagged.is_empty(), "every adapter dir has a manifest: {flagged:?}");
}
