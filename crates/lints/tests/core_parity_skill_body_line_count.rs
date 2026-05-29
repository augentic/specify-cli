//! C13 parity test: prove `CORE-005` covers the retiring
//! `skill.body-line-count` imperative predicate row using the
//! `cardinality` reserved kind.
//!
//! # Equivalence mapping
//!
//! - Imperative rule id `skill.body-line-count` ↔ declarative rule id
//!   `CORE-005`.
//! - Imperative behaviour: walk every
//!   `plugins/<plugin>/skills/<skill>/SKILL.md` under `framework_root`,
//!   strip the leading YAML frontmatter, count the resulting body
//!   lines, and emit one [`specify_lints::Diagnostic`] per skill
//!   whose body exceeds 200 lines (the global cap pinned by
//!   [`docs/standards/skill-authoring.md`](https://github.com/augentic/specify/blob/main/docs/standards/skill-authoring.md)).
//! - Declarative behaviour: the framework-profile indexer extracts
//!   one [`specify_lints::lint::Skill`] fact per well-formed SKILL.md
//!   (`crates/lints/src/lint/index/skill.rs::extract`), whose
//!   `body_line_count` field is the same count of non-frontmatter
//!   body lines the imperative row used; the `kind: cardinality`
//!   interpreter
//!   (`crates/lints/src/lint/eval/cardinality.rs::evaluate`)
//!   consumes the fact set and emits one [`Diagnostic`] per skill
//!   whose `body_line_count` exceeds 200, carrying the
//!   `(skill, path, actual, max)` shape as structured evidence.
//!
//! Because the rule-id field differs between the two passes, the
//! fingerprint-based deduplication CANNOT silently merge a
//! declarative finding with the retired imperative one during any
//! future overlap window — every parity claim is characterised by the
//! `path → body_line_count` map.
//!
//! # Option
//!
//! Option A (functional parity) against a synthetic fixture. The
//! test stages three skills under `plugins/<plugin>/skills/<skill>/SKILL.md`:
//!
//! - `long-skill` — body sized well above the 200-line cap (300 body
//!   lines), expected to be flagged by both passes.
//! - `medium-skill` — body sized well below the cap (50 body lines),
//!   negative control.
//! - `short-skill` — minimal body (3 body lines), negative control.
//!
//! Then runs:
//!
//! 1. The retiring imperative predicate's body inline (anchored as
//!    executable code in this test crate so the parity claim does not
//!    depend on the deleted module). Captures the
//!    `{ path → body_line_count }` set for every skill whose body
//!    exceeds 200 lines.
//! 2. The declarative pipeline: `lint::index::build` under the
//!    framework scan profile (which populates `model.skills` with the
//!    `body_line_count` field), then `lint::eval::evaluate` against a
//!    synthesised `CORE-005` rule carrying the two hints CORE-005
//!    ships on disk (`path-pattern: plugins/**/SKILL.md` +
//!    `cardinality: skill-body-line-count-max-200`).
//!
//! Both passes MUST agree on the `{ path → body_line_count }` set.
//! Per-finding locations are NOT compared byte-identically because
//! the retired predicate produced a `PathBuf` location stamped with
//! `line: 1` while the declarative evaluator produces a
//! project-relative string location with the actual body line count
//! surfaced through structured evidence; functional parity (which
//! skills were flagged at which line count) is the contract.
//!
//! Fixtures stay comfortably away from the 200-line boundary so the
//! one-line difference between the imperative `skill_body_lines`
//! convention (split-and-trim) and the declarative
//! `strip_frontmatter(text).lines()` convention does not flip a
//! verdict — both counts land on the same side of the cap for every
//! fixture skill.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use specify_lints::lint::ScanProfile;
use specify_lints::lint::eval::{ToolOutput, ToolRunError, ToolRunner, evaluate};
use specify_lints::lint::index::build;
use specify_lints::rules::{
    DeterministicHint, Diagnostic, FindingEvidence, HintKind, Origin, PathRoot, ResolvedRule,
    Severity,
};

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

/// Reproduces the deleted imperative `check::skill_body::check_body_line_count`
/// body inline so the parity claim is anchored to executable code in
/// this commit. Walks every `plugins/<plugin>/skills/<skill>/SKILL.md`
/// path under `project_dir`, applies the same `skill_body_lines`
/// convention the retired helper used (split the post-frontmatter
/// remainder by `\n`, drop a leading empty entry and a trailing empty
/// entry), and returns the `{ path → body_line_count }` map for every
/// skill whose body exceeds the 200-line cap.
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

/// Mirror of the retired `helpers::skill_body_lines` convention:
/// strip the leading `---\n…\n---\n` block, split the remainder by
/// `\n`, then drop a single leading empty entry and a single trailing
/// empty entry. Sufficient for the parity fixture because the
/// synthesised SKILL.md files are well-formed.
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

fn make_rule(rule_id: &str, hints: Vec<DeterministicHint>) -> ResolvedRule {
    ResolvedRule {
        rule_id: rule_id.to_string(),
        title: format!("{rule_id} parity fixture"),
        severity: Severity::Important,
        trigger: format!("Trigger for {rule_id}"),
        lint_mode: None,
        applicability: None,
        deterministic_hints: if hints.is_empty() { None } else { Some(hints) },
        references: None,
        origin: Origin::Core,
        path_root: PathRoot::RulesRoot,
        path: format!("adapters/shared/rules/core/{rule_id}.md"),
        body: String::new(),
        deprecated: None,
    }
}

fn hint(kind: HintKind, value: &str) -> DeterministicHint {
    DeterministicHint {
        kind,
        value: value.to_string(),
        description: None,
    }
}

struct NoToolRunner;

impl ToolRunner for NoToolRunner {
    fn run(
        &self, _tool_name: &str, _args: &[String], _project_dir: &Path,
    ) -> Result<ToolOutput, ToolRunError> {
        Err(ToolRunError::Runtime("no tool runner wired".to_string()))
    }

    fn is_declared(&self, _tool_name: &str) -> bool {
        false
    }
}

#[test]
fn core_005_matches_imperative_skill_body_line_count_row() {
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
        rule.deterministic_hints.as_deref().unwrap_or_default(),
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
