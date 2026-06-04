//! `CORE-003` ≅ the retiring `skill.duplicate-name` imperative row. Both group
//! `SKILL.md` files by frontmatter `name:` and flag duplicated names; the
//! `(skill_name, sorted_paths)` set must agree.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use specify_diagnostics::{Diagnostic, FindingEvidence};
use specify_standards::lint::ScanProfile;
use specify_standards::lint::eval::{ToolRunner, evaluate};
use specify_standards::lint::index::build;
use specify_standards::rules::HintKind;

use crate::eval_support::{NoToolRunner, hint, make_rule};

const DUP_A: &str =
    "---\nname: duplicate-skill\ndescription: Run the duplicate-skill flow.\n---\n# Body A\n";
const DUP_B: &str =
    "---\nname: duplicate-skill\ndescription: Run the duplicate-skill flow again.\n---\n# Body B\n";
const SOLO: &str =
    "---\nname: solo-skill\ndescription: Run the solo-skill flow.\n---\n# Body solo\n";

fn stage_project(project_dir: &Path) {
    let a = project_dir.join("plugins/alpha/skills/build/SKILL.md");
    let b = project_dir.join("plugins/beta/skills/build/SKILL.md");
    let solo = project_dir.join("plugins/gamma/skills/solo/SKILL.md");
    for parent in [a.parent(), b.parent(), solo.parent()].into_iter().flatten() {
        fs::create_dir_all(parent).expect("plugin skill dir");
    }
    fs::write(&a, DUP_A).expect("write dup A");
    fs::write(&b, DUP_B).expect("write dup B");
    fs::write(&solo, SOLO).expect("write solo");
}

/// Reproduces the deleted imperative
/// `check::skill_frontmatter::check_duplicate_names` body inline; returns
/// the `(skill_name, sorted_paths)` groups with two or more files.
fn imperative_duplicate_set(project_dir: &Path) -> BTreeMap<String, Vec<String>> {
    let mut by_name: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
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
            let Some(name) = parse_frontmatter_name(&content) else {
                continue;
            };
            let Ok(relative) = path.strip_prefix(project_dir) else { continue };
            let rel = relative.to_string_lossy().replace('\\', "/");
            by_name.entry(name).or_default().insert(rel);
        }
    }

    by_name
        .into_iter()
        .filter(|(_, paths)| paths.len() >= 2)
        .map(|(name, paths)| (name, paths.into_iter().collect::<Vec<_>>()))
        .collect()
}

/// Minimal `name:` extractor mirroring the retired imperative row.
fn parse_frontmatter_name(content: &str) -> Option<String> {
    let rest = content.strip_prefix("---\n")?;
    let end = rest.find("\n---")?;
    let block = &rest[..end];
    for line in block.lines() {
        if let Some(value) = line.strip_prefix("name:") {
            let trimmed = value.trim().trim_matches(|c: char| c == '"' || c == '\'');
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

fn declarative_duplicate_set(findings: &[Diagnostic]) -> BTreeMap<String, Vec<String>> {
    let mut out = BTreeMap::new();
    for finding in findings {
        let FindingEvidence::Structured { data, .. } = &finding.evidence else { continue };
        let name = data.get("name").and_then(|v| v.as_str()).map(str::to_string);
        let paths = data.get("paths").and_then(|v| v.as_array()).map(|arr| {
            arr.iter().filter_map(|v| v.as_str().map(str::to_string)).collect::<Vec<_>>()
        });
        if let (Some(name), Some(paths)) = (name, paths) {
            out.insert(name, paths);
        }
    }
    out
}

#[test]
fn matches_duplicate_name() {
    let project = tempfile::tempdir().expect("tempdir");
    let project_dir = project.path();
    stage_project(project_dir);

    let imperative = imperative_duplicate_set(project_dir);
    assert!(
        !imperative.is_empty(),
        "imperative row must flag the duplicate pair (parity fixture invariant)",
    );

    let mut expected: BTreeMap<String, Vec<String>> = BTreeMap::new();
    expected.insert(
        "duplicate-skill".to_string(),
        vec![
            "plugins/alpha/skills/build/SKILL.md".to_string(),
            "plugins/beta/skills/build/SKILL.md".to_string(),
        ],
    );
    assert_eq!(
        imperative, expected,
        "imperative fixture must match the documented duplicate-name set",
    );

    let model = build(project_dir, ScanProfile::Framework, &[], &[]).expect("framework build");
    let rule = make_rule(
        "CORE-003",
        vec![
            hint(HintKind::PathPattern, "plugins/**/SKILL.md"),
            hint(HintKind::Unique, "skill-name"),
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
            Some("CORE-003"),
            "declarative findings must carry the documented CORE-003 rule id",
        );
        let loc = finding.location.as_ref().expect("location set");
        assert!(
            loc.path.starts_with("plugins/"),
            "declarative location path must point at a `plugins/**/SKILL.md` file: got {}",
            loc.path,
        );
    }

    let declarative = declarative_duplicate_set(&outcome.findings);
    assert_eq!(
        declarative, imperative,
        "declarative CORE-003 must flag the same (name, paths) pairs as the retired skill.duplicate-name predicate",
    );
}
