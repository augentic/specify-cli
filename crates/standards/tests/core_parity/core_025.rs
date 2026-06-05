//! `CORE-025` — operational vocabulary with path-pattern exclusions (RFC-31 Phase 2).

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::sync::OnceLock;

use regex::Regex;
use specify_diagnostics::Diagnostic;
use specify_standards::lint::ScanProfile;
use specify_standards::lint::eval::evaluate;
use specify_standards::lint::index::build;
use specify_standards::rules::HintKind;

use crate::eval_support::{NoToolRunner, hint, make_rule};

fn forbidden_patterns() -> Vec<Regex> {
    static PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();
    PATTERNS
        .get_or_init(|| {
            [
                r"\.specify/changes/",
                r"\bspecify validate\b",
                r"\bspecify merge\b",
                r"\bspecify change plan\b",
                r"\bspecify change draft\b",
                r"\b[Ii]nitiative\b",
            ]
            .into_iter()
            .map(|p| Regex::new(p).expect("forbidden pattern"))
            .collect()
        })
        .clone()
}

fn stage_project(project_dir: &Path) {
    fs::create_dir_all(project_dir.join("docs/explanation")).expect("mkdir explanation");
    fs::create_dir_all(project_dir.join("docs/fixtures/nested")).expect("mkdir fixtures");
    fs::write(project_dir.join("docs/bad.md"), "Run specify validate on this slice.\n")
        .expect("write bad");
    fs::write(
        project_dir.join("docs/explanation/decision-log.md"),
        "Run specify validate here too (allowlisted path).\n",
    )
    .expect("write decision-log");
    fs::write(
        project_dir.join("docs/fixtures/nested/x.md"),
        "Run specify validate under fixtures (segment allowlist).\n",
    )
    .expect("write fixtures");
}

fn imperative_flagged_lines(project_dir: &Path) -> BTreeSet<(String, u32)> {
    let allowed_prefixes =
        ["docs/explanation/decision-log.md", "docs/explanation/release-notes.md"];
    let allowed_segments = ["/fixtures/", "/archive/"];
    let patterns = forbidden_patterns();
    let mut out = BTreeSet::new();
    let rel = "docs/bad.md";
    let path = project_dir.join(rel);
    let content = fs::read_to_string(&path).expect("read");
    for (line_idx, line) in content.lines().enumerate() {
        if allowed_prefixes.iter().any(|p| rel.starts_with(p)) {
            continue;
        }
        if allowed_segments.iter().any(|s| rel.contains(s)) {
            continue;
        }
        if patterns.iter().any(|re| re.is_match(line)) {
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
fn core_025_exclusion_parity() {
    let dir = tempfile::tempdir().expect("tempdir");
    stage_project(dir.path());

    let imperative = imperative_flagged_lines(dir.path());
    assert_eq!(imperative.len(), 1);

    let model = build(dir.path(), ScanProfile::Product, &[], &[]).expect("index");
    let mut hints = vec![
        hint(HintKind::PathPattern, "docs/**/*.md"),
        hint(HintKind::PathPattern, "!docs/explanation/decision-log.md"),
        hint(HintKind::PathPattern, "!docs/explanation/release-notes.md"),
        hint(HintKind::PathPattern, "!**/fixtures/**"),
        hint(HintKind::PathPattern, "!**/archive/**"),
    ];
    for pattern in [
        r"\.specify/changes/",
        r"\bspecify validate\b",
        r"\bspecify merge\b",
        r"\bspecify change plan\b",
        r"\bspecify change draft\b",
        r"\b[Ii]nitiative\b",
    ] {
        hints.push(hint(HintKind::Regex, pattern));
    }
    let rule = make_rule("CORE-025", hints);
    let outcome = evaluate(
        &rule,
        rule.rule_hints.as_deref().unwrap_or_default(),
        &model,
        dir.path(),
        &NoToolRunner,
        1,
    )
    .expect("declarative evaluate");

    let declarative = declarative_flagged_lines(&outcome.findings);
    assert_eq!(
        declarative, imperative,
        "declarative exclusions + regex must flag the same (file, line) set as imperative carve-outs",
    );
}
