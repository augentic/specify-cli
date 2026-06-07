//! Integration test for the `set-eq` hint evaluator.
//!
//! Exercises the config-driven `adapter-briefs` source — an adapter
//! manifest's `briefs.keys()` must exactly equal the axis-appropriate
//! operation set the rule supplies in `config: { expected-operations }`,
//! flagging both `missing` operations and `unexpected` keys — over a
//! framework model, with no reference to any specify rule id. The
//! expected operation sets are policy supplied by the rule's `config`,
//! never a `const` in the engine arm.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde_json::json;
use specify_diagnostics::FindingEvidence;
use specify_standards::lint::ScanProfile;
use specify_standards::lint::eval::{ToolRunner, evaluate};
use specify_standards::lint::index::build;
use specify_standards::rules::{HintKind, RuleHint};

use crate::eval_support::{NoToolRunner, hint, hint_with_config, make_rule};

fn write_manifest(project: &Path, rel: &str, body: &str) {
    let path = project.join(rel);
    fs::create_dir_all(path.parent().expect("parent")).expect("manifest dir");
    fs::write(&path, body).expect("write manifest");
}

/// `(adapter, divergence, operation)` triples surfaced by the evaluator.
fn divergences(project: &Path, hints: Vec<RuleHint>) -> BTreeSet<(String, String, String)> {
    let model = build(project, ScanProfile::Framework, &[], &[]).expect("framework build");
    let rule = make_rule("UNI-950", hints);
    let runner: &dyn ToolRunner = &NoToolRunner;
    let outcome =
        evaluate(&rule, rule.rule_hints.as_deref().unwrap_or_default(), &model, project, runner, 1)
            .expect("evaluate");
    outcome
        .findings
        .iter()
        .filter_map(|f| match &f.evidence {
            FindingEvidence::Structured { data, .. } => {
                let adapter = data.get("adapter")?.as_str()?.to_string();
                let divergence = data.get("divergence")?.as_str()?.to_string();
                let operation = data.get("operation")?.as_str()?.to_string();
                Some((adapter, divergence, operation))
            }
            _ => None,
        })
        .collect()
}

fn hints() -> Vec<RuleHint> {
    vec![
        hint(HintKind::PathPattern, "adapters/sources/*/adapter.yaml"),
        hint(HintKind::PathPattern, "adapters/targets/*/adapter.yaml"),
        hint_with_config(
            HintKind::SetEq,
            "adapter-briefs",
            Some(json!({
                "expected-operations": {
                    "sources": ["survey", "extract"],
                    "targets": ["shape", "build", "merge"],
                }
            })),
        ),
    ]
}

#[test]
fn flags_missing_and_unexpected_operations() {
    let tmp = tempfile::tempdir().expect("tmp");
    write_manifest(
        tmp.path(),
        "adapters/sources/bad-source/adapter.yaml",
        "name: bad-source\nversion: 1\naxis: source\ndescription: Missing extract.\nbriefs:\n  survey: briefs/survey.md\n",
    );
    write_manifest(
        tmp.path(),
        "adapters/targets/bad-target/adapter.yaml",
        "name: bad-target\nversion: 1\naxis: target\ndescription: Missing merge, stray key.\nbriefs:\n  shape: briefs/shape.md\n  build: briefs/build.md\n  extra: briefs/extra.md\n",
    );

    let flagged = divergences(tmp.path(), hints());
    let expected: BTreeSet<(String, String, String)> = [
        ("bad-source", "missing", "extract"),
        ("bad-target", "missing", "merge"),
        ("bad-target", "unexpected", "extra"),
    ]
    .into_iter()
    .map(|(a, d, o)| (a.to_string(), d.to_string(), o.to_string()))
    .collect();
    assert_eq!(
        flagged, expected,
        "both halves of the symmetric difference are flagged per (adapter, divergence, operation)",
    );
}

#[test]
fn exact_set_passes() {
    let tmp = tempfile::tempdir().expect("tmp");
    write_manifest(
        tmp.path(),
        "adapters/targets/good-target/adapter.yaml",
        "name: good-target\nversion: 1\naxis: target\ndescription: Exact operation set.\nbriefs:\n  shape: briefs/shape.md\n  build: briefs/build.md\n  merge: briefs/merge.md\n",
    );

    let flagged = divergences(tmp.path(), hints());
    assert!(flagged.is_empty(), "an exact operation set produces no findings: {flagged:?}");
}
