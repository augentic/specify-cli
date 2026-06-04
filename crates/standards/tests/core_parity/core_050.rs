//! `CORE-050` — retired `specify-contract` without `-validate` suffix via `regex` config.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use ::regex::Regex;
use specify_diagnostics::Diagnostic;
use specify_standards::lint::ScanProfile;
use specify_standards::lint::eval::evaluate;
use specify_standards::lint::index::build;
use specify_standards::rules::HintKind;

use crate::eval_support::{NoToolRunner, hint_with_config, make_rule};

fn stage_project(project_dir: &Path) {
    let skill = project_dir.join("plugins/spec/skills/init/SKILL.md");
    fs::create_dir_all(skill.parent().expect("parent")).expect("mkdir");
    fs::write(&skill, "Run specify-contract-validate here\nAlso specify-contract alone\n")
        .expect("write skill");
}

fn imperative_flagged_lines(project_dir: &Path) -> BTreeSet<(String, u32)> {
    let pattern = Regex::new(r"\bspecify-contract\b").expect("regex");
    let rel = "plugins/spec/skills/init/SKILL.md";
    let path = project_dir.join(rel);
    let content = fs::read_to_string(&path).expect("read");
    let mut out = BTreeSet::new();
    for (line_idx, line) in content.lines().enumerate() {
        if pattern.find_iter(line).any(|m| !line[m.end()..].starts_with("-validate")) {
            out.insert((rel.to_string(), u32::try_from(line_idx + 1).unwrap_or(u32::MAX)));
        }
    }
    out
}

fn declarative_flagged_lines(findings: &[Diagnostic]) -> BTreeSet<(String, u32)> {
    findings
        .iter()
        .filter_map(|f| {
            let loc = f.location.as_ref()?;
            Some((loc.path.clone(), loc.line?))
        })
        .collect()
}

#[test]
fn core_050_suffix_parity() {
    let dir = tempfile::tempdir().expect("tempdir");
    stage_project(dir.path());

    let imperative = imperative_flagged_lines(dir.path());
    assert_eq!(imperative.len(), 1);

    let model = build(dir.path(), ScanProfile::Framework, &[], &[]).expect("index");
    let rule = make_rule(
        "CORE-050",
        vec![
            hint_with_config(HintKind::PathPattern, "plugins/**/skills/**/SKILL.md", None),
            hint_with_config(
                HintKind::Regex,
                r"\bspecify-contract\b",
                Some(serde_json::json!({ "suffix-must-not-start-with": "-validate" })),
            ),
        ],
    );
    let runner = NoToolRunner;
    let outcome = evaluate(
        &rule,
        rule.rule_hints.as_deref().unwrap_or_default(),
        &model,
        dir.path(),
        &runner,
        1,
    )
    .expect("declarative evaluate");

    let declarative = declarative_flagged_lines(&outcome.findings);
    assert_eq!(
        declarative, imperative,
        "declarative suffix guard must flag the same (file, line) set as the imperative reference",
    );
}
