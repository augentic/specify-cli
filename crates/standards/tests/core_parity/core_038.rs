//! `CORE-038` — `## Input` restatement via `regex` on skill paths.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use specify_diagnostics::Diagnostic;
use specify_standards::lint::ScanProfile;
use specify_standards::lint::eval::evaluate;
use specify_standards::lint::index::build;
use specify_standards::rules::HintKind;

use crate::eval_support::{NoToolRunner, hint, make_rule};

fn stage_project(project_dir: &Path) {
    let skill = project_dir.join("plugins/spec/skills/init/SKILL.md");
    fs::create_dir_all(skill.parent().expect("parent")).expect("mkdir");
    fs::write(
        &skill,
        "---\nname: init\ndescription: Test skill for input restatement parity.\n---\n\n## Input\n\nRestated args.\n",
    )
    .expect("write skill");
}

fn imperative_flagged_lines(project_dir: &Path) -> BTreeSet<(String, u32)> {
    let rel = "plugins/spec/skills/init/SKILL.md";
    let content = fs::read_to_string(project_dir.join(rel)).expect("read");
    let mut out = BTreeSet::new();
    for (idx, line) in content.lines().enumerate() {
        if line.trim() == "## Input" {
            out.insert((rel.to_string(), u32::try_from(idx + 1).unwrap_or(u32::MAX)));
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
fn core_038_input_restatement_parity() {
    let dir = tempfile::tempdir().expect("tempdir");
    stage_project(dir.path());
    let imperative = imperative_flagged_lines(dir.path());
    assert_eq!(imperative.len(), 1);

    let model = build(dir.path(), ScanProfile::Framework, &[], &[]).expect("index");
    let rule = make_rule(
        "CORE-038",
        vec![
            hint(HintKind::PathPattern, "plugins/**/skills/**/SKILL.md"),
            hint(HintKind::Regex, r"(?m)^## Input\s*$"),
        ],
    );
    let outcome = evaluate(
        &rule,
        rule.rule_hints.as_deref().unwrap_or_default(),
        &model,
        dir.path(),
        &NoToolRunner,
        1,
    )
    .expect("eval");
    assert_eq!(declarative_flagged_lines(&outcome.findings), imperative);
}
