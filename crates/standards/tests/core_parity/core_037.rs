//! `CORE-037` — envelope JSON in skill body via `fenced-block` facts (RFC-31 Phase 2 pilot).

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::sync::LazyLock;

use regex::{Regex, RegexBuilder};
use specify_diagnostics::Diagnostic;
use specify_standards::lint::ScanProfile;
use specify_standards::lint::eval::evaluate;
use specify_standards::lint::index::build;
use specify_standards::rules::HintKind;

use crate::eval_support::{NoToolRunner, hint, make_rule};

static ENVELOPE_FENCE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(`{3,})(json|jsonc)\b").expect("envelope fence"));
static ENVELOPE_VERSION_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""envelope[-_]version"\s*:"#).expect("envelope version"));
static ENVELOPE_OK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""ok"\s*:\s*(true|false)\b"#).expect("envelope ok"));
static ENVELOPE_DATA_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""data"\s*:"#).expect("envelope data"));
static ENVELOPE_ERROR_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""error"\s*:\s*\{"#).expect("envelope error"));

fn stage_project(project_dir: &Path) {
    let skill = project_dir.join("plugins/spec/skills/init/SKILL.md");
    fs::create_dir_all(skill.parent().expect("parent")).expect("mkdir");
    fs::write(
        &skill,
        "---\nname: init\ndescription: Test skill for envelope parity.\n---\n\n```json\n{\"ok\": true, \"data\": {}, \"envelope_version\": \"1\"}\n```\n",
    )
    .expect("write skill");
}

fn is_envelope_body(body: &[String]) -> bool {
    let text = body.join("\n");
    if ENVELOPE_VERSION_RE.is_match(&text) {
        return true;
    }
    let has_ok = ENVELOPE_OK_RE.is_match(&text);
    let has_data = ENVELOPE_DATA_RE.is_match(&text);
    let has_error = ENVELOPE_ERROR_RE.is_match(&text);
    has_ok && (has_data || has_error)
}

fn imperative_flagged_lines(project_dir: &Path) -> BTreeSet<(String, u32)> {
    let rel = "plugins/spec/skills/init/SKILL.md";
    let path = project_dir.join(rel);
    let content = fs::read_to_string(&path).expect("read");
    let lines: Vec<&str> = content.split('\n').collect();
    let mut out = BTreeSet::new();
    let mut in_block = false;
    let mut block_start = 0_usize;
    let mut block_body: Vec<String> = Vec::new();
    let mut open_fence: Option<String> = None;

    for (i, line) in lines.iter().enumerate() {
        if !in_block {
            if let Some(caps) = ENVELOPE_FENCE_RE.captures(line) {
                in_block = true;
                open_fence = Some(caps[1].to_string());
                block_start = i + 1;
                block_body.clear();
            }
            continue;
        }
        let fence = open_fence.as_deref().unwrap_or("```");
        let close_re =
            RegexBuilder::new(&format!(r"^\s*{fence}\s*$")).build().expect("fence close");
        if close_re.is_match(line) {
            if is_envelope_body(&block_body) {
                out.insert((rel.to_string(), u32::try_from(block_start + 1).unwrap_or(u32::MAX)));
            }
            in_block = false;
            open_fence = None;
            block_body.clear();
            continue;
        }
        block_body.push((*line).to_string());
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
fn core_037_envelope_parity() {
    let dir = tempfile::tempdir().expect("tempdir");
    stage_project(dir.path());

    let imperative = imperative_flagged_lines(dir.path());
    assert_eq!(imperative.len(), 1);

    let model = build(dir.path(), ScanProfile::Framework, &[], &[]).expect("index");
    let rule = make_rule(
        "CORE-037",
        vec![
            hint(HintKind::PathPattern, "plugins/**/skills/**/SKILL.md"),
            hint(HintKind::FencedBlock, "skill-envelope-json-in-body"),
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
    .expect("declarative evaluate");

    let declarative = declarative_flagged_lines(&outcome.findings);
    assert_eq!(
        declarative, imperative,
        "fenced-block envelope predicate must flag the same (file, line) set as imperative",
    );
}
