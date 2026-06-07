//! Integration test for the `field-grammar` hint evaluator.
//!
//! Exercises the two mechanism modes over a framework model with no
//! reference to any real `CORE-NNN`:
//!
//! - `field-tokens` — a candidate whose named frontmatter field carries
//!   a whitespace token that fails the `token-pattern` regex is flagged;
//!   one whose tokens all match passes.
//! - `field-first-word` — a candidate whose named field begins with a
//!   word outside the `allowed` list is flagged; one beginning with an
//!   allowed word passes.
//!
//! Every value (the field name, the grammar regex, the allow-list) is
//! policy supplied by the rule's `config`, never a `const` in the
//! engine arm.

use std::fs;
use std::path::Path;

use serde_json::json;
use specify_diagnostics::FindingEvidence;
use specify_standards::lint::ScanProfile;
use specify_standards::lint::eval::{ToolRunner, evaluate};
use specify_standards::lint::index::build;
use specify_standards::rules::{HintKind, RuleHint};

use crate::eval_support::{NoToolRunner, hint, hint_with_config, make_rule};

fn write_skill(project: &Path, plugin: &str, skill: &str, content: &str) {
    let path = project.join(format!("plugins/{plugin}/skills/{skill}/SKILL.md"));
    fs::create_dir_all(path.parent().expect("parent")).expect("skill dir");
    fs::write(&path, content).expect("write skill");
}

fn flagged_paths(project: &Path, rule_id: &str, hints: Vec<RuleHint>) -> Vec<String> {
    let model = build(project, ScanProfile::Framework, &[], &[]).expect("framework build");
    let rule = make_rule(rule_id, hints);
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

#[test]
fn tokens_flags_bad_and_passes_good() {
    let tmp = tempfile::tempdir().expect("tmp");
    write_skill(
        tmp.path(),
        "good",
        "hint",
        "---\nname: hint\nargument-hint: <slice-dir> [crate-name]\n---\n\nBody.\n",
    );
    write_skill(
        tmp.path(),
        "bad",
        "hint",
        "---\nname: hint\nargument-hint: the slice name\n---\n\nBody.\n",
    );

    let flagged = flagged_paths(
        tmp.path(),
        "UNI-980",
        vec![
            hint(HintKind::PathPattern, "plugins/**/SKILL.md"),
            hint_with_config(
                HintKind::FieldGrammar,
                "field-tokens",
                Some(
                    json!({ "field": "argument-hint", "token-pattern": r"^[<\[][a-z][a-z0-9-]*[>\]]$" }),
                ),
            ),
        ],
    );
    assert_eq!(
        flagged,
        vec!["plugins/bad/skills/hint/SKILL.md".to_string()],
        "the prose argument-hint is flagged; the grammar-conformant one passes",
    );
}

#[test]
fn first_word_flags_bad_and_passes_good() {
    let tmp = tempfile::tempdir().expect("tmp");
    write_skill(
        tmp.path(),
        "good",
        "desc",
        "---\nname: desc\ndescription: Build the demo fixtures.\n---\n\nBody.\n",
    );
    write_skill(
        tmp.path(),
        "bad",
        "desc",
        "---\nname: desc\ndescription: The thing that does work.\n---\n\nBody.\n",
    );

    let flagged = flagged_paths(
        tmp.path(),
        "UNI-981",
        vec![
            hint(HintKind::PathPattern, "plugins/**/SKILL.md"),
            hint_with_config(
                HintKind::FieldGrammar,
                "field-first-word",
                Some(json!({ "field": "description", "allowed": ["build", "run"] })),
            ),
        ],
    );
    assert_eq!(
        flagged,
        vec!["plugins/bad/skills/desc/SKILL.md".to_string()],
        "the non-verb description is flagged; the allowed-verb one passes",
    );
}
