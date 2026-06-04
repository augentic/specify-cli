//! `CORE-005` ≅ the retiring `skill.body-line-count` imperative row via the
//! `cardinality` kind: SKILL.md bodies over 200 lines are flagged. The
//! `{ path -> body_line_count }` set must agree.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use specify_diagnostics::{Diagnostic, FindingEvidence};
use specify_standards::lint::ScanProfile;
use specify_standards::lint::eval::{ToolRunner, evaluate};
use specify_standards::lint::index::build;
use specify_standards::rules::HintKind;

use crate::eval_support::{NoToolRunner, hint, make_rule};

const SKILL_BODY_LINE_MAX: u32 = 200;

fn write_skill(project_dir: &Path, plugin: &str, skill: &str, name: &str, body_lines: u32) {
    let body = (1..=body_lines).map(|i| format!("body line {i}")).collect::<Vec<_>>().join("\n");
    let content = format!(
        "---\nname: {name}\ndescription: Fixture skill for the CORE-005 parity test.\nargument-hint: <arg>\n---\n{body}\n",
    );
    let path = project_dir.join(format!("plugins/{plugin}/skills/{skill}/SKILL.md"));
    fs::create_dir_all(path.parent().expect("parent")).expect("plugin skill dir");
    fs::write(&path, content).expect("write skill");
}

fn stage_project(project_dir: &Path) {
    write_skill(project_dir, "alpha", "long", "long-skill", 300);
    write_skill(project_dir, "beta", "medium", "medium-skill", 50);
    write_skill(project_dir, "gamma", "short", "short-skill", 3);
}

/// Reproduces the deleted imperative
/// `check::skill_body::check_body_line_count` body inline; returns the
/// `{ path -> body_line_count }` map for skills over the 200-line cap.
fn imperative_over_cap_set(project_dir: &Path) -> BTreeMap<String, usize> {
    let mut out = BTreeMap::new();
    let plugins = project_dir.join("plugins");
    let mut stack: Vec<PathBuf> = vec![plugins];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.file_name().and_then(|s| s.to_str()) != Some("SKILL.md") {
                continue;
            }
            let Ok(content) = fs::read_to_string(&path) else { continue };
            let Some(lines) = imperative_body_lines(&content) else { continue };
            if lines.len() > SKILL_BODY_LINE_MAX as usize {
                let Ok(relative) = path.strip_prefix(project_dir) else { continue };
                let rel = relative.to_string_lossy().replace('\\', "/");
                out.insert(rel, lines.len());
            }
        }
    }
    out
}

/// Mirror of the retired `helpers::skill_body_lines` convention.
fn imperative_body_lines(content: &str) -> Option<Vec<String>> {
    let rest = content.strip_prefix("---\n")?;
    let end = rest.find("\n---")?;
    let block = &rest[..end];
    let start = content.find(block)? + block.len();
    let mut lines: Vec<String> = content[start..].split('\n').map(str::to_string).collect();
    if lines.first().is_some_and(String::is_empty) {
        lines.remove(0);
    }
    if lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    Some(lines)
}

fn declarative_over_cap_set(findings: &[Diagnostic]) -> BTreeMap<String, u32> {
    let mut out = BTreeMap::new();
    for finding in findings {
        let FindingEvidence::Structured { data, .. } = &finding.evidence else { continue };
        let path = data.get("path").and_then(|v| v.as_str()).map(str::to_string);
        let actual = data
            .get("actual")
            .and_then(serde_json::Value::as_u64)
            .and_then(|n| u32::try_from(n).ok());
        if let (Some(path), Some(actual)) = (path, actual) {
            out.insert(path, actual);
        }
    }
    out
}

#[test]
fn matches_body_line_count() {
    let project = tempfile::tempdir().expect("tempdir");
    let project_dir = project.path();
    stage_project(project_dir);

    let imperative = imperative_over_cap_set(project_dir);
    assert_eq!(
        imperative.len(),
        1,
        "imperative row must flag exactly the long-skill fixture (parity fixture invariant)",
    );
    let long_path = "plugins/alpha/skills/long/SKILL.md".to_string();
    assert!(imperative.contains_key(&long_path), "imperative must flag {long_path}");
    assert!(imperative[&long_path] > SKILL_BODY_LINE_MAX as usize);

    let model = build(project_dir, ScanProfile::Framework, &[], &[]).expect("framework build");
    let rule = make_rule(
        "CORE-005",
        vec![
            hint(HintKind::PathPattern, "plugins/**/SKILL.md"),
            hint(HintKind::Cardinality, "skill-body-line-count-max-200"),
        ],
    );
    let runner: &dyn ToolRunner = &NoToolRunner;
    let outcome = evaluate(
        &rule,
        rule.rule_hints.as_deref().unwrap_or_default(),
        &model,
        project_dir,
        runner,
        1,
    )
    .expect("declarative evaluate");

    for finding in &outcome.findings {
        assert_eq!(
            finding.rule_id.as_deref(),
            Some("CORE-005"),
            "declarative findings must carry the documented CORE-005 rule id",
        );
        let loc = finding.location.as_ref().expect("location set");
        assert!(
            loc.path.starts_with("plugins/"),
            "declarative location path must point at a `plugins/**/SKILL.md` file: got {}",
            loc.path,
        );
    }

    let declarative = declarative_over_cap_set(&outcome.findings);
    assert_eq!(
        declarative.len(),
        imperative.len(),
        "declarative CORE-005 must flag the same number of skills as the retired skill.body-line-count predicate",
    );
    let declarative_paths: Vec<&String> = declarative.keys().collect();
    let imperative_paths: Vec<&String> = imperative.keys().collect();
    assert_eq!(
        declarative_paths, imperative_paths,
        "declarative CORE-005 must flag the same paths as the retired skill.body-line-count predicate",
    );
    for path in declarative.keys() {
        let declarative_count = declarative[path];
        let imperative_count = imperative[path];
        assert!(
            declarative_count > SKILL_BODY_LINE_MAX,
            "declarative count for {path} must exceed the cap ({declarative_count})",
        );
        assert!(
            imperative_count > SKILL_BODY_LINE_MAX as usize,
            "imperative count for {path} must exceed the cap ({imperative_count})",
        );
    }
}
