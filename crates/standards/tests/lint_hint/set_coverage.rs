//! Integration test for the `set-coverage` hint evaluator.
//!
//! Exercises the config-driven `skill-allowed-tools` source — every
//! declared `allowed-tools` entry must be covered by `config.allowed` or
//! match an `config.allowed-prefixes` exemption — over a framework
//! model, with no reference to any specify rule id.

use std::fs;
use std::path::Path;

use serde_json::json;
use specify_diagnostics::FindingEvidence;
use specify_standards::lint::ScanProfile;
use specify_standards::lint::eval::{ToolRunner, evaluate};
use specify_standards::lint::index::build;
use specify_standards::rules::{HintKind, RuleHint};

use crate::eval_support::{NoToolRunner, hint, hint_with_config, make_rule};

fn write_skill(project: &Path, plugin: &str, skill: &str, name: &str, allowed_tools: &str) {
    let content = format!(
        "---\nname: {name}\ndescription: Fixture.\nallowed-tools: {allowed_tools}\n---\n\n# Body\n",
    );
    let path = project.join(format!("plugins/{plugin}/skills/{skill}/SKILL.md"));
    fs::create_dir_all(path.parent().expect("parent")).expect("skill dir");
    fs::write(&path, content).expect("write skill");
}

fn flagged_tools(project: &Path, hints: Vec<RuleHint>) -> Vec<String> {
    let model = build(project, ScanProfile::Framework, &[], &[]).expect("framework build");
    let rule = make_rule("UNI-920", hints);
    let runner: &dyn ToolRunner = &NoToolRunner;
    let outcome =
        evaluate(&rule, rule.rule_hints.as_deref().unwrap_or_default(), &model, project, runner, 1)
            .expect("evaluate");
    let mut tools: Vec<String> = outcome
        .findings
        .iter()
        .filter_map(|f| match &f.evidence {
            FindingEvidence::Structured { data, .. } => {
                data.get("tool").and_then(|v| v.as_str()).map(str::to_string)
            }
            _ => None,
        })
        .collect();
    tools.sort();
    tools
}

#[test]
fn flags_only_uncovered_tools() {
    let tmp = tempfile::tempdir().expect("tmp");
    write_skill(tmp.path(), "p", "s", "p-s", "Read NotATool mcp__server__do");

    let flagged = flagged_tools(
        tmp.path(),
        vec![
            hint(HintKind::PathPattern, "plugins/**/SKILL.md"),
            hint_with_config(
                HintKind::SetCoverage,
                "skill-allowed-tools",
                Some(json!({ "allowed": ["Read"], "allowed-prefixes": ["mcp__"] })),
            ),
        ],
    );
    assert_eq!(
        flagged,
        vec!["NotATool".to_string()],
        "only the unrecognised tool is flagged; allow-listed and prefix-exempt tools pass",
    );
}

#[test]
fn skills_without_allowed_tools_pass() {
    let tmp = tempfile::tempdir().expect("tmp");
    let content = "---\nname: p-s\ndescription: Fixture.\n---\n\n# Body\n";
    let path = tmp.path().join("plugins/p/skills/s/SKILL.md");
    fs::create_dir_all(path.parent().expect("parent")).expect("dir");
    fs::write(&path, content).expect("write");

    let flagged = flagged_tools(
        tmp.path(),
        vec![
            hint(HintKind::PathPattern, "plugins/**/SKILL.md"),
            hint_with_config(
                HintKind::SetCoverage,
                "skill-allowed-tools",
                Some(json!({ "allowed": ["Read"] })),
            ),
        ],
    );
    assert!(flagged.is_empty(), "a skill that declares no tools is never flagged: {flagged:?}");
}
