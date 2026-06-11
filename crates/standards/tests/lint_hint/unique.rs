//! Integration test for the `unique` hint evaluator.
//!
//! Exercises the config-driven `skill` source with `config: { field }` —
//! every `name:` value declared across `plugins/**/SKILL.md` must be
//! unique; a name shared by two or more files is flagged — over a
//! framework model, with no reference to any specify rule id. The field
//! selector is policy supplied by the rule's `config`, never a `const`
//! discriminator in the engine arm.

use std::fs;
use std::path::Path;

use serde_json::json;
use specify_diagnostics::FindingEvidence;
use specify_standards::lint::ScanProfile;
use specify_standards::lint::eval::{ToolRunner, evaluate};
use specify_standards::lint::index::build;
use specify_standards::rules::{HintKind, RuleHint};

use crate::eval_support::{NoToolRunner, hint, hint_with_config, make_rule};

fn write_skill(project: &Path, plugin: &str, skill: &str, name: &str) {
    let content = format!("---\nname: {name}\ndescription: Fixture.\n---\n\n# Body\n");
    let path = project.join(format!("plugins/{plugin}/skills/{skill}/SKILL.md"));
    fs::create_dir_all(path.parent().expect("parent")).expect("skill dir");
    fs::write(&path, content).expect("write skill");
}

fn write_scenario(project: &Path, name: &str, id: &str) {
    let content = format!(
        "---\nid: {id}\nowner: spec\nkind: skill\nentrypoint: /spec:refine\nstages: [refine, build]\nisolation: fresh-project\n---\n\nBody.\n"
    );
    let path = project.join(format!("evals/scenarios/{name}"));
    fs::create_dir_all(path.parent().expect("parent")).expect("scenario dir");
    fs::write(&path, content).expect("write scenario");
}

fn duplicate_ids(project: &Path, hints: Vec<RuleHint>) -> Vec<String> {
    let model = build(project, ScanProfile::Framework, &[], &[]).expect("framework build");
    let rule = make_rule("UNI-961", hints);
    let runner: &dyn ToolRunner = &NoToolRunner;
    let outcome =
        evaluate(&rule, rule.rule_hints.as_deref().unwrap_or_default(), &model, project, runner, 1)
            .expect("evaluate");
    let mut ids: Vec<String> = outcome
        .findings
        .iter()
        .filter_map(|f| match &f.evidence {
            FindingEvidence::Structured { data, .. } => {
                data.get("id").and_then(|v| v.as_str()).map(str::to_string)
            }
            _ => None,
        })
        .collect();
    ids.sort();
    ids
}

fn duplicate_names(project: &Path, hints: Vec<RuleHint>) -> Vec<String> {
    let model = build(project, ScanProfile::Framework, &[], &[]).expect("framework build");
    let rule = make_rule("UNI-960", hints);
    let runner: &dyn ToolRunner = &NoToolRunner;
    let outcome =
        evaluate(&rule, rule.rule_hints.as_deref().unwrap_or_default(), &model, project, runner, 1)
            .expect("evaluate");
    let mut names: Vec<String> = outcome
        .findings
        .iter()
        .filter_map(|f| match &f.evidence {
            FindingEvidence::Structured { data, .. } => {
                data.get("name").and_then(|v| v.as_str()).map(str::to_string)
            }
            _ => None,
        })
        .collect();
    names.sort();
    names
}

#[test]
fn flags_duplicated_field_value() {
    let tmp = tempfile::tempdir().expect("tmp");
    write_skill(tmp.path(), "alpha", "build", "shared-name");
    write_skill(tmp.path(), "beta", "build", "shared-name");
    write_skill(tmp.path(), "gamma", "solo", "unique-name");

    let flagged = duplicate_names(
        tmp.path(),
        vec![
            hint(HintKind::PathPattern, "plugins/**/SKILL.md"),
            hint_with_config(HintKind::Unique, "skill", Some(json!({ "field": "skill-name" }))),
        ],
    );
    assert_eq!(
        flagged,
        vec!["shared-name".to_string()],
        "only the name shared by two files is flagged; the solo name passes",
    );
}

#[test]
fn distinct_values_pass() {
    let tmp = tempfile::tempdir().expect("tmp");
    write_skill(tmp.path(), "alpha", "build", "alpha-build");
    write_skill(tmp.path(), "beta", "build", "beta-build");

    let flagged = duplicate_names(
        tmp.path(),
        vec![
            hint(HintKind::PathPattern, "plugins/**/SKILL.md"),
            hint_with_config(HintKind::Unique, "skill", Some(json!({ "field": "skill-name" }))),
        ],
    );
    assert!(flagged.is_empty(), "all-distinct names produce no findings: {flagged:?}");
}

#[test]
fn flags_duplicate_scenario_id() {
    let tmp = tempfile::tempdir().expect("tmp");
    write_scenario(tmp.path(), "a.md", "shared-id");
    write_scenario(tmp.path(), "b.md", "shared-id");
    write_scenario(tmp.path(), "c.md", "solo-id");

    let flagged = duplicate_ids(
        tmp.path(),
        vec![hint_with_config(HintKind::Unique, "scenario", Some(json!({ "field": "id" })))],
    );
    assert_eq!(
        flagged,
        vec!["shared-id".to_string()],
        "only the id shared by two files is flagged; the solo id passes",
    );
}

#[test]
fn distinct_scenario_ids_pass() {
    let tmp = tempfile::tempdir().expect("tmp");
    write_scenario(tmp.path(), "a.md", "alpha-id");
    write_scenario(tmp.path(), "b.md", "beta-id");

    let flagged = duplicate_ids(
        tmp.path(),
        vec![hint_with_config(HintKind::Unique, "scenario", Some(json!({ "field": "id" })))],
    );
    assert!(flagged.is_empty(), "all-distinct scenario ids produce no findings: {flagged:?}");
}
