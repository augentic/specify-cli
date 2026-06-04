//! `CORE-023` — slash-skill positional via `regex` config + logical-line join.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use specify_diagnostics::Diagnostic;
use specify_standards::lint::ScanProfile;
use specify_standards::lint::eval::evaluate;
use specify_standards::lint::eval::regex::logical_lines::{
    logical_lines_with_starts, violates_slash_skill_positional,
};
use specify_standards::lint::index::build;
use specify_standards::rules::HintKind;

use crate::eval_support::{NoToolRunner, hint_with_config, make_rule};

fn stage_project(project_dir: &Path) {
    fs::create_dir_all(project_dir.join("docs")).expect("docs");
    fs::write(project_dir.join("docs/bad.md"), "/spec:build \\\n  --retry\n").expect("write bad");
}

fn imperative_flagged_lines(project_dir: &Path) -> BTreeSet<(String, u32)> {
    let rel = "docs/bad.md";
    let content = fs::read_to_string(project_dir.join(rel)).expect("read");
    let mut out = BTreeSet::new();
    for (start, logical) in logical_lines_with_starts(&content) {
        if violates_slash_skill_positional(&logical) {
            out.insert((rel.to_string(), u32::try_from(start).unwrap_or(u32::MAX)));
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
fn core_023_slash_skill_parity() {
    let dir = tempfile::tempdir().expect("tempdir");
    stage_project(dir.path());
    let imperative = imperative_flagged_lines(dir.path());
    assert_eq!(imperative.len(), 1);

    let model = build(dir.path(), ScanProfile::Consumer, &[], &[]).expect("index");
    let rule = make_rule(
        "CORE-023",
        vec![
            hint_with_config(HintKind::PathPattern, "docs/**/*.md", None),
            hint_with_config(
                HintKind::Regex,
                "slash-skill-positional",
                Some(serde_json::json!({
                    "slash-skill-positional": true,
                    "join-backslash-continuations": true
                })),
            ),
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
