//! C10 parity test: prove `CORE-002` covers the retiring `links.unresolved`
//! imperative predicate row.
//!
//! # Equivalence mapping
//!
//! - Imperative rule id `links.unresolved` ↔ declarative rule id `CORE-002`.
//! - Imperative location: absolute `Path` to the markdown file containing the
//!   broken link (`line: 1` is a placeholder — the old predicate did not
//!   record the link line).
//! - Declarative location: project-relative path string with the 1-based
//!   link line populated from the `WorkspaceModel.markdown_links` fact.
//! - Imperative emitted one [`specify_standards::Diagnostic`] per broken
//!   `[label](target)` link after fence / inline-code / HTML-comment
//!   stripping. The declarative `reference-resolves` evaluator consumes
//!   the same fence-aware link facts the indexer already extracted
//!   (`crates/standards/src/lint/index/markdown.rs::extract_links`)
//!   and folds the resolver result (`resolves: Some(false)`) from the
//!   sequential indexer pass (`crates/standards/src/lint/index.rs::resolve_link`).
//!
//! The two implementations share the same link grammar (`[label](target)`),
//! the same fence-skipping behaviour, and the same path-resolution rule
//! (join `target` against the markdown file's parent directory and check
//! the joined path against the discovered file set). Because the rule-id
//! field differs, the fingerprint-based deduplication CANNOT
//! silently merge a declarative finding with the retired imperative one
//! during any future overlap window — every parity claim is characterised
//! by the `(from_path, target)` pair.
//!
//! # Option
//!
//! Option A (functional parity). The test stages a fixture with one
//! markdown file whose links resolve (`docs/valid.md` linking to a sibling
//! that exists) and one broken file (`docs/broken.md` linking to a path
//! that does not exist), then runs:
//!
//! 1. The retiring imperative predicate's body inline (anchored as
//!    executable code in this test crate so the parity claim does not
//!    depend on the deleted module). Captures the
//!    `(from_path_relative, target)` set per file.
//! 2. The declarative pipeline: `lint::index::build` under the framework
//!    scan profile (which populates `markdown_links.resolves`), then
//!    `lint::eval::evaluate` against a synthesised `CORE-002` rule
//!    carrying the same two hints CORE-002 ships on disk
//!    (`path-pattern: docs/**/*.md` + `reference-resolves: markdown-link`).
//!
//! Both passes MUST agree on the `(file, broken_target)` set. Locations
//! are NOT compared byte-identically because the retired predicate
//! reported `line: 1` while the declarative evaluator reports the actual
//! link line; functional parity (which references were flagged in which
//! files) is the contract.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;
use specify_diagnostics::{Diagnostic, FindingEvidence, Severity};
use specify_standards::lint::ScanProfile;
use specify_standards::lint::eval::{ToolOutput, ToolRunError, ToolRunner, evaluate};
use specify_standards::lint::index::build;
use specify_standards::rules::{DeterministicHint, HintKind, Origin, PathRoot, ResolvedRule};

const VALID_BODY: &str =
    "# Valid page\n\nSee [target](./target.md) and [external](https://example.com).\n";
const TARGET_BODY: &str = "# Target\n";
const BROKEN_BODY: &str =
    "# Broken page\n\nSee [missing](./missing.md) and [also-missing](../other/absent.md).\n";

fn stage_project(project_dir: &Path) {
    let docs = project_dir.join("docs");
    fs::create_dir_all(&docs).expect("docs dir");
    fs::write(docs.join("target.md"), TARGET_BODY).expect("write target");
    fs::write(docs.join("valid.md"), VALID_BODY).expect("write valid");
    fs::write(docs.join("broken.md"), BROKEN_BODY).expect("write broken");
}

/// Reproduces the deleted imperative `check::links::check_markdown_links`
/// body inline so the parity claim is anchored to executable code in
/// this commit. Mirrors the regex pattern, the fence-stripping pass,
/// and the URL / anchor / `src/` short-circuits verbatim; returns the
/// set of `(from_relative, target)` pairs that would have been flagged.
fn imperative_broken_set(project_dir: &Path) -> BTreeSet<(String, String)> {
    let link_re = link_pattern();
    let fence_re = fenced_code_pattern();
    let inline_re = inline_code_pattern();
    let comment_re = html_comment_pattern();

    let mut out: BTreeSet<(String, String)> = BTreeSet::new();
    let mut stack: Vec<PathBuf> = vec![project_dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|s| s.to_str()) != Some("md") {
                continue;
            }
            let Ok(content) = fs::read_to_string(&path) else { continue };
            let parent = path.parent().unwrap_or(project_dir);
            let stripped = {
                let no_fence = fence_re.replace_all(&content, "");
                let no_comments = comment_re.replace_all(&no_fence, "");
                inline_re.replace_all(&no_comments, "").into_owned()
            };
            for cap in link_re.captures_iter(&stripped) {
                let target = cap.get(1).map_or("", |m| m.as_str());
                if target.starts_with("http://")
                    || target.starts_with("https://")
                    || target.starts_with("mailto:")
                    || target.starts_with('#')
                {
                    continue;
                }
                let path_part = target.split('#').next().unwrap_or("");
                if path_part.is_empty() || path_part.starts_with("src/") {
                    continue;
                }
                if !parent.join(path_part).exists() {
                    let rel = path.strip_prefix(project_dir).map_or_else(
                        |_| path.display().to_string(),
                        |p| p.to_string_lossy().replace('\\', "/"),
                    );
                    out.insert((rel, target.to_string()));
                }
            }
        }
    }
    out
}

fn declarative_broken_set(findings: &[Diagnostic]) -> BTreeSet<(String, String)> {
    findings
        .iter()
        .filter_map(|f| {
            let loc = f.location.as_ref()?;
            let target = match &f.evidence {
                FindingEvidence::Snippet { value } => value.clone(),
                _ => return None,
            };
            Some((loc.path.clone(), target))
        })
        .collect()
}

fn link_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\[[^\]]*\]\(([^)]+)\)").expect("valid link pattern"))
}

fn fenced_code_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"```[\s\S]*?```").expect("valid fence pattern"))
}

fn inline_code_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"`[^`]+`").expect("valid inline code pattern"))
}

fn html_comment_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?s)<!--.*?-->").expect("valid comment pattern"))
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
fn core_002_matches_imperative_markdown_link_row() {
    let project = tempfile::tempdir().expect("tempdir");
    let project_dir = project.path();
    stage_project(project_dir);

    let imperative = imperative_broken_set(project_dir);
    assert!(
        !imperative.is_empty(),
        "imperative row must flag the broken page (parity fixture invariant)"
    );

    let expected: BTreeSet<(String, String)> = [
        ("docs/broken.md".to_string(), "./missing.md".to_string()),
        ("docs/broken.md".to_string(), "../other/absent.md".to_string()),
    ]
    .into_iter()
    .collect();
    assert_eq!(
        imperative, expected,
        "imperative fixture must match the documented broken-link set",
    );

    let model = build(project_dir, ScanProfile::Framework, &[], &[]).expect("framework build");
    let rule = make_rule(
        "CORE-002",
        vec![
            hint(HintKind::PathPattern, "docs/**/*.md"),
            hint(HintKind::ReferenceResolves, "markdown-link"),
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
            Some("CORE-002"),
            "declarative findings must carry the documented CORE-002 rule id",
        );
        let loc = finding.location.as_ref().expect("location set");
        assert!(
            loc.line.is_some_and(|line| line >= 1),
            "declarative finding must record the 1-based link line",
        );
    }

    let declarative = declarative_broken_set(&outcome.findings);
    assert_eq!(
        declarative, imperative,
        "declarative CORE-002 must flag the same (file, target) pairs as the retired links.unresolved predicate",
    );
}
