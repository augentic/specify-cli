//! Integration test for the `fenced-block` hint evaluator.
//!
//! Exercises the config-driven `inline-json-too-long` source — fences
//! whose info string is one of `config.langs` and whose body exceeds
//! `config.max-lines` — and the `fenced-body-contains` source — fences
//! whose info string is in `config.langs` and whose body contains a
//! banned `config.substrings` token — over a framework model, with no
//! reference to any specify rule id.

use std::fs;
use std::path::Path;

use serde_json::json;
use specify_diagnostics::FindingEvidence;
use specify_standards::lint::ScanProfile;
use specify_standards::lint::eval::{ToolRunner, evaluate};
use specify_standards::lint::index::build;
use specify_standards::rules::{HintKind, RuleHint};

use crate::eval_support::{NoToolRunner, hint, hint_with_config, make_rule};

fn write_skill(project: &Path, body: &str) {
    let content = format!("---\nname: p-s\ndescription: Fixture.\n---\n\n{body}\n");
    let path = project.join("plugins/p/skills/s/SKILL.md");
    fs::create_dir_all(path.parent().expect("parent")).expect("skill dir");
    fs::write(&path, content).expect("write skill");
}

fn flagged_lines(project: &Path, hints: Vec<RuleHint>) -> Vec<u64> {
    let model = build(project, ScanProfile::Framework, &[], &[]).expect("framework build");
    let rule = make_rule("UNI-930", hints);
    let runner: &dyn ToolRunner = &NoToolRunner;
    let outcome =
        evaluate(&rule, rule.rule_hints.as_deref().unwrap_or_default(), &model, project, runner, 1)
            .expect("evaluate");
    let mut lines: Vec<u64> = outcome
        .findings
        .iter()
        .filter_map(|f| match &f.evidence {
            FindingEvidence::Structured { data, .. } => {
                data.get("line-start").and_then(serde_json::Value::as_u64)
            }
            _ => None,
        })
        .collect();
    lines.sort_unstable();
    lines
}

#[test]
fn flags_long_json_fences_only() {
    let tmp = tempfile::tempdir().expect("tmp");
    let body = "```json\n1\n2\n3\n4\n5\n```\n\n```json\nx\n```\n";
    write_skill(tmp.path(), body);

    let flagged = flagged_lines(
        tmp.path(),
        vec![
            hint(HintKind::PathPattern, "plugins/**/SKILL.md"),
            hint_with_config(
                HintKind::FencedBlock,
                "inline-json-too-long",
                Some(json!({ "langs": ["json", "jsonc"], "max-lines": 3 })),
            ),
        ],
    );
    assert_eq!(flagged.len(), 1, "only the 5-line json fence is flagged: {flagged:?}");
}

#[test]
fn ignores_fences_outside_lang_set() {
    let tmp = tempfile::tempdir().expect("tmp");
    let body = "```text\n1\n2\n3\n4\n5\n6\n```\n";
    write_skill(tmp.path(), body);

    let flagged = flagged_lines(
        tmp.path(),
        vec![
            hint(HintKind::PathPattern, "plugins/**/SKILL.md"),
            hint_with_config(
                HintKind::FencedBlock,
                "inline-json-too-long",
                Some(json!({ "langs": ["json"], "max-lines": 3 })),
            ),
        ],
    );
    assert!(flagged.is_empty(), "a long `text` fence is out of the lang set: {flagged:?}");
}

#[test]
fn flags_text_fence_with_banned_substring() {
    let tmp = tempfile::tempdir().expect("tmp");
    let body = "```text\nA -> B\n```\n\n```text\njust prose\n```\n";
    write_skill(tmp.path(), body);

    let flagged = flagged_lines(
        tmp.path(),
        vec![
            hint(HintKind::PathPattern, "plugins/**/SKILL.md"),
            hint_with_config(
                HintKind::FencedBlock,
                "fenced-body-contains",
                Some(json!({ "langs": ["text"], "substrings": ["->", "→"] })),
            ),
        ],
    );
    assert_eq!(flagged.len(), 1, "only the arrow-bearing text fence is flagged: {flagged:?}");
}

#[test]
fn ignores_substring_outside_lang_set() {
    let tmp = tempfile::tempdir().expect("tmp");
    let body = "```rust\nlet _ = a -> b;\n```\n";
    write_skill(tmp.path(), body);

    let flagged = flagged_lines(
        tmp.path(),
        vec![
            hint(HintKind::PathPattern, "plugins/**/SKILL.md"),
            hint_with_config(
                HintKind::FencedBlock,
                "fenced-body-contains",
                Some(json!({ "langs": ["text"], "substrings": ["->"] })),
            ),
        ],
    );
    assert!(flagged.is_empty(), "an arrow in a `rust` fence is out of the lang set: {flagged:?}");
}
