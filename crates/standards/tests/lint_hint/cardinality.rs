//! Integration test for the `cardinality` hint evaluator.
//!
//! Exercises the config-driven `skill-body-line-count` metric (whole-skill
//! body cap), the `markdown-h2-section-body-line-count` metric — the
//! per-section line cap read from `config.max`, scoped to level-2 sections
//! — and the scope-aware `brief-parent-body-line-count` /
//! `brief-phase-body-line-count` metrics, over a framework model, with no
//! reference to any specify rule id. Every metric reads its cap from
//! `config.max`; none embeds a numeric cap in the engine arm.

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

fn write_skill(project: &Path, plugin: &str, skill: &str, name: &str, body: &str) {
    let content = format!("---\nname: {name}\ndescription: Fixture.\n---\n\n{body}\n");
    let path = project.join(format!("plugins/{plugin}/skills/{skill}/SKILL.md"));
    fs::create_dir_all(path.parent().expect("parent")).expect("skill dir");
    fs::write(&path, content).expect("write skill");
}

fn write_brief(project: &Path, rel: &str, lines: usize) {
    let body: String = (1..=lines).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
    let path = project.join(rel);
    fs::create_dir_all(path.parent().expect("parent")).expect("brief dir");
    fs::write(&path, format!("# Brief\n\n{body}\n")).expect("write brief");
}

fn flagged_paths(project: &Path, hints: Vec<RuleHint>) -> Vec<String> {
    let model = build(project, ScanProfile::Framework, &[], &[]).expect("framework build");
    let rule = make_rule("UNI-912", hints);
    let runner: &dyn ToolRunner = &NoToolRunner;
    let outcome =
        evaluate(&rule, rule.rule_hints.as_deref().unwrap_or_default(), &model, project, runner, 1)
            .expect("evaluate");
    let mut paths: Vec<String> =
        outcome.findings.iter().filter_map(|f| Some(f.location.as_ref()?.path.clone())).collect();
    paths.sort();
    paths
}

fn flagged_titles(project: &Path, hints: Vec<RuleHint>) -> Vec<String> {
    let model = build(project, ScanProfile::Framework, &[], &[]).expect("framework build");
    let rule = make_rule("UNI-910", hints);
    let runner: &dyn ToolRunner = &NoToolRunner;
    let outcome =
        evaluate(&rule, rule.rule_hints.as_deref().unwrap_or_default(), &model, project, runner, 1)
            .expect("evaluate");
    let mut titles: Vec<String> = outcome
        .findings
        .iter()
        .filter_map(|f| match &f.evidence {
            FindingEvidence::Structured { data, .. } => {
                data.get("title").and_then(|v| v.as_str()).map(str::to_string)
            }
            _ => None,
        })
        .collect();
    titles.sort();
    titles
}

#[test]
fn flags_skill_body_over_cap() {
    let tmp = tempfile::tempdir().expect("tmp");
    write_skill(tmp.path(), "p", "big", "p-big", "l1\nl2\nl3\nl4\nl5\nl6");
    write_skill(tmp.path(), "p", "small", "p-small", "only one line");

    let flagged = flagged_paths(
        tmp.path(),
        vec![
            hint(HintKind::PathPattern, "plugins/**/SKILL.md"),
            hint_with_config(
                HintKind::Cardinality,
                "skill-body-line-count",
                Some(json!({ "max": 3 })),
            ),
        ],
    );
    assert_eq!(
        flagged,
        vec!["plugins/p/skills/big/SKILL.md".to_string()],
        "only the over-cap skill body is flagged by the whole-skill metric",
    );
}

#[test]
fn flags_h2_sections_over_cap() {
    let tmp = tempfile::tempdir().expect("tmp");
    let body = "## Big\nl1\nl2\nl3\nl4\n\n## Small\nl1\n";
    write_skill(tmp.path(), "p", "s", "p-s", body);

    let flagged = flagged_titles(
        tmp.path(),
        vec![
            hint(HintKind::PathPattern, "plugins/**/SKILL.md"),
            hint_with_config(
                HintKind::Cardinality,
                "markdown-h2-section-body-line-count",
                Some(json!({ "max": 3 })),
            ),
        ],
    );
    assert_eq!(flagged, vec!["Big".to_string()], "only the over-cap H2 section is flagged");
}

#[test]
fn ignores_non_h2_sections() {
    let tmp = tempfile::tempdir().expect("tmp");
    let body = "## Top\n### Deep\nl1\nl2\nl3\nl4\nl5\n";
    write_skill(tmp.path(), "p", "s", "p-s", body);

    let flagged = flagged_titles(
        tmp.path(),
        vec![
            hint(HintKind::PathPattern, "plugins/**/SKILL.md"),
            hint_with_config(
                HintKind::Cardinality,
                "markdown-h2-section-body-line-count",
                Some(json!({ "max": 1 })),
            ),
        ],
    );
    assert!(
        !flagged.contains(&"Deep".to_string()),
        "the over-cap H3 subsection must not be flagged: {flagged:?}",
    );
}

#[test]
fn brief_scope_metrics_flag_per_scope() {
    let tmp = tempfile::tempdir().expect("tmp");
    // Parent brief over a 3-line cap; phase sub-brief under it.
    write_brief(tmp.path(), "adapters/targets/demo/briefs/build.md", 5);
    write_brief(tmp.path(), "adapters/targets/demo/briefs/build/phase.md", 2);

    let parent_flagged = flagged_paths(
        tmp.path(),
        vec![hint_with_config(
            HintKind::Cardinality,
            "brief-parent-body-line-count",
            Some(json!({ "max": 3 })),
        )],
    );
    assert_eq!(
        parent_flagged,
        vec!["adapters/targets/demo/briefs/build.md".to_string()],
        "only the over-cap parent brief is flagged by the parent metric",
    );

    // Phase metric with a cap that the phase sub-brief clears but a
    // parent brief would exceed — proving scope isolation.
    let phase_flagged = flagged_paths(
        tmp.path(),
        vec![hint_with_config(
            HintKind::Cardinality,
            "brief-phase-body-line-count",
            Some(json!({ "max": 1 })),
        )],
    );
    assert_eq!(
        phase_flagged,
        vec!["adapters/targets/demo/briefs/build/phase.md".to_string()],
        "the phase metric only flags phase sub-briefs, not the over-cap parent",
    );
}

#[test]
fn missing_config_is_unsupported() {
    let tmp = tempfile::tempdir().expect("tmp");
    write_skill(tmp.path(), "p", "s", "p-s", "## Big\nl1\nl2\n");

    let model = build(tmp.path(), ScanProfile::Framework, &[], &[]).expect("build");
    let rule = make_rule(
        "UNI-911",
        vec![
            hint(HintKind::PathPattern, "plugins/**/SKILL.md"),
            hint(HintKind::Cardinality, "markdown-h2-section-body-line-count"),
        ],
    );
    let runner: &dyn ToolRunner = &NoToolRunner;
    let result = evaluate(
        &rule,
        rule.rule_hints.as_deref().unwrap_or_default(),
        &model,
        tmp.path(),
        runner,
        1,
    );
    assert!(result.is_err(), "a config-driven metric without config must be rejected");
}
