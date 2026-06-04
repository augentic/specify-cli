//! `CORE-014` — brief frontmatter forbidden via `path-pattern` + line-1 `regex`.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use specify_diagnostics::Diagnostic;
use specify_standards::framework::check::brief::{is_parent_brief, is_phase_sub_brief};
use specify_standards::lint::ScanProfile;
use specify_standards::lint::eval::evaluate;
use specify_standards::lint::index::build;
use specify_standards::rules::HintKind;

use crate::eval_support::{NoToolRunner, hint, make_rule};

fn stage_project(project_dir: &Path) {
    fs::create_dir_all(project_dir.join("adapters/targets/demo/briefs")).expect("briefs");
    fs::write(
        project_dir.join("adapters/targets/demo/briefs/extract.md"),
        "---\ndescription: drift\n---\n\n# Extract\n",
    )
    .expect("write brief");
    fs::write(
        project_dir.join("adapters/targets/demo/briefs/shape.md"),
        "# Shape\n\nNo frontmatter here.\n",
    )
    .expect("write shape");
}

fn imperative_flagged(project_dir: &Path) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let rel = "adapters/targets/demo/briefs/extract.md";
    if !is_parent_brief(rel) && !is_phase_sub_brief(rel) {
        return out;
    }
    let content = fs::read_to_string(project_dir.join(rel)).expect("read");
    if content.starts_with("---\n") || content.starts_with("---\r\n") {
        out.insert(rel.to_string());
    }
    out
}

fn declarative_flagged(findings: &[Diagnostic]) -> BTreeSet<String> {
    findings.iter().filter_map(|f| f.location.as_ref().map(|l| l.path.clone())).collect()
}

#[test]
fn core_014_brief_frontmatter_parity() {
    let dir = tempfile::tempdir().expect("tempdir");
    stage_project(dir.path());
    let imperative = imperative_flagged(dir.path());
    assert_eq!(imperative.len(), 1);

    let model = build(dir.path(), ScanProfile::Framework, &[], &[]).expect("index");
    let rule = make_rule(
        "CORE-014",
        vec![
            hint(HintKind::PathPattern, "adapters/**/briefs/extract.md"),
            hint(HintKind::PathPattern, "adapters/**/briefs/shape.md"),
            hint(HintKind::Regex, "^---"),
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
    assert_eq!(declarative_flagged(&outcome.findings), imperative);
}
