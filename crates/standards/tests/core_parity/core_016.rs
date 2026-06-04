//! `CORE-016` — design-history citation (`RFC-N` where `N < 100`) via `regex` config.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use specify_diagnostics::Diagnostic;
use specify_standards::lint::ScanProfile;
use specify_standards::lint::eval::evaluate;
use specify_standards::lint::index::build;
use specify_standards::rules::HintKind;

use crate::eval_support::{NoToolRunner, hint_with_config, make_rule};

fn stage_project(project_dir: &Path) {
    fs::create_dir_all(project_dir.join("docs")).expect("docs dir");
    fs::write(project_dir.join("docs/bad.md"), "See RFC-5 for retired design-history citation.\n")
        .expect("write bad");
    fs::write(
        project_dir.join("docs/good.md"),
        "Use RFC 3339 timestamps and RFC 5322 email syntax.\n",
    )
    .expect("write good");
}

fn imperative_flagged_lines(project_dir: &Path) -> BTreeSet<(String, u32)> {
    let mut out = BTreeSet::new();
    for rel in ["docs/bad.md", "docs/good.md"] {
        let path = project_dir.join(rel);
        let content = fs::read_to_string(&path).expect("read");
        for (line_idx, line) in content.lines().enumerate() {
            if has_specify_history_citation(line) {
                out.insert((rel.to_string(), u32::try_from(line_idx + 1).unwrap_or(u32::MAX)));
            }
        }
    }
    out
}

fn has_specify_history_citation(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    let retired_tree = ["rfc", "s/"].concat();
    let retired_token = ["rfc", "-"].concat();
    if lower.contains(&retired_tree) || contains_numbered_token(&lower, &retired_token) {
        return true;
    }

    let mut search = text;
    let retired_upper = ["R", "FC"].concat();
    while let Some(idx) = search.find(&retired_upper) {
        let rest = &search[idx + retired_upper.len()..];
        if let Some(number) =
            parse_design_history_number(rest.strip_prefix('-').or_else(|| rest.strip_prefix(' ')))
            && number < 100
        {
            return true;
        }
        search = advance_one(rest);
    }
    false
}

fn contains_numbered_token(text: &str, token: &str) -> bool {
    let mut search = text;
    while let Some(idx) = search.find(token) {
        let rest = &search[idx + token.len()..];
        if parse_design_history_number(Some(rest)).is_some_and(|number| number < 100) {
            return true;
        }
        search = advance_one(rest);
    }
    false
}

fn advance_one(text: &str) -> &str {
    text.char_indices().nth(1).map_or("", |(idx, _)| &text[idx..])
}

fn parse_design_history_number(rest: Option<&str>) -> Option<u32> {
    let rest = rest?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() { None } else { digits.parse().ok() }
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
fn core_016_regex_parity() {
    let dir = tempfile::tempdir().expect("tempdir");
    stage_project(dir.path());

    let imperative = imperative_flagged_lines(dir.path());
    assert_eq!(imperative.len(), 1, "fixture must flag exactly one line");

    let model = build(dir.path(), ScanProfile::Framework, &[], &[]).expect("index");
    let rule = make_rule(
        "CORE-016",
        vec![
            hint_with_config(HintKind::PathPattern, "docs/**/*.md", None),
            hint_with_config(
                HintKind::Regex,
                r"(?i)RFC[-\s]+(\d+)",
                Some(serde_json::json!({
                    "capture-group": 1,
                    "capture-op": "lt",
                    "capture-value": 100
                })),
            ),
        ],
    );
    let runner = NoToolRunner;
    let outcome = evaluate(
        &rule,
        rule.rule_hints.as_deref().unwrap_or_default(),
        &model,
        dir.path(),
        &runner,
        1,
    )
    .expect("declarative evaluate");

    let declarative = declarative_flagged_lines(&outcome.findings);
    assert_eq!(
        declarative, imperative,
        "declarative regex config must flag the same (file, line) set as the imperative reference",
    );
}
