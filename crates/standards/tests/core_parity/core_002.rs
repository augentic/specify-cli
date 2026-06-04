//! `CORE-002` ≅ the retiring `links.unresolved` imperative row. Both share the
//! `[label](target)` grammar, fence-skipping, and the parent-relative
//! path-resolution rule; the `(file, broken_target)` set must agree.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;
use specify_diagnostics::{Diagnostic, FindingEvidence};
use specify_standards::lint::ScanProfile;
use specify_standards::lint::eval::{ToolRunner, evaluate};
use specify_standards::lint::index::build;
use specify_standards::rules::HintKind;

use crate::eval_support::{NoToolRunner, hint, make_rule};

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
/// body inline; returns the `(from_relative, target)` pairs that would
/// have been flagged.
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

#[test]
fn matches_imperative_markdown_link_row() {
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
