//! Integration test for the `constant-eq` hint evaluator.
//!
//! Exercises the config-driven `skill-name-plugin-prefix` source —
//! every well-formed skill `name` must begin with its plugin's prefix
//! (`<plugin>-`), modulo `config.overrides` — over a framework model,
//! with no reference to any specify rule id.

mod eval_support;

use std::fs;
use std::path::Path;

use eval_support::{NoToolRunner, hint, hint_with_config, make_rule};
use serde_json::json;
use specify_diagnostics::FindingEvidence;
use specify_standards::lint::ScanProfile;
use specify_standards::lint::eval::{ToolRunner, evaluate};
use specify_standards::lint::index::build;
use specify_standards::rules::{HintKind, RuleHint};

fn write_skill(project: &Path, plugin: &str, skill: &str, name: &str) {
    let content = format!("---\nname: {name}\ndescription: Fixture.\n---\n\n# Body\n");
    let path = project.join(format!("plugins/{plugin}/skills/{skill}/SKILL.md"));
    fs::create_dir_all(path.parent().expect("parent")).expect("skill dir");
    fs::write(&path, content).expect("write skill");
}

fn flagged_skills(project: &Path, hints: Vec<RuleHint>) -> Vec<String> {
    let model = build(project, ScanProfile::Framework, &[], &[]).expect("framework build");
    let rule = make_rule("UNI-940", hints);
    let runner: &dyn ToolRunner = &NoToolRunner;
    let outcome =
        evaluate(&rule, rule.rule_hints.as_deref().unwrap_or_default(), &model, project, runner, 1)
            .expect("evaluate");
    let mut names: Vec<String> = outcome
        .findings
        .iter()
        .filter_map(|f| match &f.evidence {
            FindingEvidence::Structured { data, .. } => {
                data.get("skill").and_then(|v| v.as_str()).map(str::to_string)
            }
            _ => None,
        })
        .collect();
    names.sort();
    names
}

#[test]
fn flags_names_missing_plugin_prefix() {
    let tmp = tempfile::tempdir().expect("tmp");
    write_skill(tmp.path(), "alpha", "good", "alpha-good");
    write_skill(tmp.path(), "alpha", "bad", "wrong-name");

    let flagged = flagged_skills(
        tmp.path(),
        vec![
            hint(HintKind::PathPattern, "plugins/**/SKILL.md"),
            hint_with_config(
                HintKind::ConstantEq,
                "skill-name-plugin-prefix",
                Some(json!({ "overrides": {} })),
            ),
        ],
    );
    assert_eq!(flagged, vec!["wrong-name".to_string()], "only the mismatched name is flagged");
}

#[test]
fn honours_override_prefix() {
    let tmp = tempfile::tempdir().expect("tmp");
    write_skill(tmp.path(), "spec", "init", "specify-init");

    let flagged = flagged_skills(
        tmp.path(),
        vec![
            hint(HintKind::PathPattern, "plugins/**/SKILL.md"),
            hint_with_config(
                HintKind::ConstantEq,
                "skill-name-plugin-prefix",
                Some(json!({ "overrides": { "spec": "specify" } })),
            ),
        ],
    );
    assert!(flagged.is_empty(), "the overridden prefix accepts `specify-`: {flagged:?}");
}
