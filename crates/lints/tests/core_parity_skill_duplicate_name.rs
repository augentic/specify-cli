//! C11 parity test: prove `CORE-003` covers the retiring `skill.duplicate-name`
//! imperative predicate row.
//!
//! # Equivalence mapping
//!
//! - Imperative rule id `skill.duplicate-name` ↔ declarative rule id `CORE-003`.
//! - Imperative behaviour: walk every `plugins/<plugin>/skills/<skill>/SKILL.md`
//!   under `framework_root`, read the YAML frontmatter, group files by their
//!   `name:` field, and emit one [`specify_authoring::Finding`] per duplicated
//!   name with the comma-joined offender list in the message and the first
//!   path (sorted by insertion order from the walk, which sorts entries
//!   lexically) carried in the location.
//! - Declarative behaviour: the framework-profile indexer extracts one
//!   [`specify_lints::lint::Skill`] fact per well-formed SKILL.md
//!   (`crates/lints/src/lint/index/skill.rs::extract`); the
//!   `kind: unique` interpreter
//!   (`crates/lints/src/lint/eval/unique.rs::evaluate`) consumes
//!   the same fact set and emits one [`Diagnostic`] per duplicated
//!   `name:` value, carrying the sorted offender path list as structured
//!   evidence.
//!
//! Because the rule-id field differs between the two passes, RFC-34 §F5's
//! fingerprint-based deduplication CANNOT silently merge a declarative
//! finding with the retired imperative one during any future overlap
//! window — every parity claim is characterised by the
//! `(skill_name, sorted_paths)` pair.
//!
//! # Option
//!
//! Option A (functional parity). The test stages a fixture under a
//! synthetic `plugins/<plugin>/skills/<skill>/SKILL.md` tree carrying
//! one duplicate-name pair (`duplicate-skill` appears on two files in
//! different plugins) and one unique entry (`solo-skill` appears once),
//! then runs:
//!
//! 1. The retiring imperative predicate's body inline (anchored as
//!    executable code in this test crate so the parity claim does not
//!    depend on the deleted module). Captures the
//!    `(skill_name, sorted_paths)` set per duplicate group.
//! 2. The declarative pipeline: `lint::index::build` under the
//!    framework scan profile (which populates `model.skills`), then
//!    `lint::eval::evaluate` against a synthesised `CORE-003` rule
//!    carrying the same two hints CORE-003 ships on disk
//!    (`path-pattern: plugins/**/SKILL.md` + `unique: skill-name`).
//!
//! Both passes MUST agree on the `(skill_name, sorted_paths)` set.
//! Per-finding locations are NOT compared byte-identically because the
//! retired predicate produced a `PathBuf` location stamped with `line: 1`
//! while the declarative evaluator produces a project-relative string
//! location with the duplicate-name list surfaced through structured
//! evidence; functional parity (which names were flagged with which
//! offender sets) is the contract.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use specify_lints::lint::ScanProfile;
use specify_lints::lint::eval::{ToolOutput, ToolRunError, ToolRunner, evaluate};
use specify_lints::lint::index::build;
use specify_lints::rules::{
    DeterministicHint, Diagnostic, FindingEvidence, HintKind, Origin, PathRoot, ResolvedRule,
    Severity,
};

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

/// Reproduces the deleted imperative `check::skill_frontmatter::check_duplicate_names`
/// body inline so the parity claim is anchored to executable code in
/// this commit. Walks every `plugins/<plugin>/skills/<skill>/SKILL.md`
/// path under `project_dir`, parses the leading YAML frontmatter, and
/// returns the `(skill_name, sorted_paths)` set the predicate would
/// have flagged (groups with two or more contributing files).
fn imperative_duplicate_set(project_dir: &Path) -> BTreeMap<String, Vec<String>> {
    let mut by_name: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let plugins = project_dir.join("plugins");
    let mut stack: Vec<PathBuf> = vec![plugins.clone()];
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

/// Minimal `name:` extractor mirroring the retired imperative row:
/// strip the leading `---\n…\n---\n` block, then scan the lines for a
/// `name: <value>` entry. Sufficient for the parity fixture because
/// the synthesised SKILL.md files are well-formed.
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
fn core_003_matches_imperative_skill_duplicate_name_row() {
    let project = tempfile::tempdir().expect("tempdir");
    let project_dir = project.path();
    stage_project(project_dir);

    let imperative = imperative_duplicate_set(project_dir);
    assert!(
        !imperative.is_empty(),
        "imperative row must flag the duplicate pair (parity fixture invariant)",
    );

    let expected: BTreeMap<String, Vec<String>> = [(
        "duplicate-skill".to_string(),
        vec![
            "plugins/alpha/skills/build/SKILL.md".to_string(),
            "plugins/beta/skills/build/SKILL.md".to_string(),
        ],
    )]
    .into_iter()
    .collect();
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
