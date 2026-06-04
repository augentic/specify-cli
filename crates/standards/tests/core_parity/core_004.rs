//! `CORE-004` ≅ the `set-coverage` reserved-kind semantics: adapter manifest
//! `briefs.keys()` must cover the axis-appropriate operation enum. No
//! imperative `Check` row is retired; an inline reference stands in.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use specify_diagnostics::{Diagnostic, FindingEvidence};
use specify_standards::lint::ScanProfile;
use specify_standards::lint::eval::{ToolRunner, evaluate};
use specify_standards::lint::index::build;
use specify_standards::rules::HintKind;

use crate::eval_support::{NoToolRunner, hint, make_rule};

const BAD_SOURCE: &str = concat!(
    "name: bad-source\n",
    "version: 1\n",
    "axis: source\n",
    "description: Source adapter missing `extract:`.\n",
    "briefs:\n",
    "  survey: briefs/survey.md\n",
);
const BAD_TARGET: &str = concat!(
    "name: bad-target\n",
    "version: 1\n",
    "axis: target\n",
    "description: Target adapter missing `merge:`.\n",
    "briefs:\n",
    "  shape: briefs/shape.md\n",
    "  build: briefs/build.md\n",
);
const GOOD_SOURCE: &str = concat!(
    "name: good-source\n",
    "version: 1\n",
    "axis: source\n",
    "description: Fully covered source adapter (negative control).\n",
    "briefs:\n",
    "  survey: briefs/survey.md\n",
    "  extract: briefs/extract.md\n",
);

const SOURCE_OPERATIONS: &[&str] = &["extract", "survey"];
const TARGET_OPERATIONS: &[&str] = &["build", "merge", "shape"];

fn stage_project(project_dir: &Path) {
    fs::create_dir_all(project_dir.join("plugins")).expect("plugins");
    fs::create_dir_all(project_dir.join("adapters/sources")).expect("sources");
    fs::create_dir_all(project_dir.join("adapters/targets")).expect("targets");

    for (rel, body) in [
        ("adapters/sources/bad-source/adapter.yaml", BAD_SOURCE),
        ("adapters/targets/bad-target/adapter.yaml", BAD_TARGET),
        ("adapters/sources/good-source/adapter.yaml", GOOD_SOURCE),
    ] {
        let path = project_dir.join(rel);
        fs::create_dir_all(path.parent().expect("parent")).expect("manifest dir");
        fs::write(&path, body).expect("write manifest");
    }
}

/// Inline reference mirroring `kind: set-coverage`; returns the
/// `(adapter, axis, missing-operation)` triple set.
fn imperative_missing_set(project_dir: &Path) -> BTreeSet<(String, String, String)> {
    let mut out = BTreeSet::new();
    for axis in ["sources", "targets"] {
        let axis_dir = project_dir.join("adapters").join(axis);
        let Ok(entries) = fs::read_dir(&axis_dir) else { continue };
        let expected: BTreeSet<&'static str> = match axis {
            "sources" => SOURCE_OPERATIONS.iter().copied().collect(),
            "targets" => TARGET_OPERATIONS.iter().copied().collect(),
            _ => unreachable!(),
        };
        for entry in entries.flatten() {
            let dir = entry.path();
            if !dir.is_dir() {
                continue;
            }
            let manifest_path = dir.join("adapter.yaml");
            let Ok(body) = fs::read_to_string(&manifest_path) else { continue };
            let (name, keys) = parse_manifest(&body);
            let Some(name) = name else { continue };
            for op in &expected {
                if !keys.contains(*op) {
                    out.insert((name.clone(), axis.to_string(), (*op).to_string()));
                }
            }
        }
    }
    out
}

/// Minimal manifest parser: `name:` scalar plus the keys of the top-level
/// `briefs:` map.
fn parse_manifest(body: &str) -> (Option<String>, BTreeSet<String>) {
    let mut name: Option<String> = None;
    let mut keys: BTreeSet<String> = BTreeSet::new();
    let mut in_briefs = false;
    for raw in body.lines() {
        if let Some(stripped) = raw.strip_prefix("name:") {
            let trimmed = stripped.trim().trim_matches(|c: char| c == '"' || c == '\'');
            if !trimmed.is_empty() {
                name = Some(trimmed.to_string());
            }
            in_briefs = false;
            continue;
        }
        if raw == "briefs:" || raw.starts_with("briefs:") {
            in_briefs = true;
            continue;
        }
        if in_briefs {
            if raw.starts_with(' ') || raw.starts_with('\t') {
                let line = raw.trim_start();
                if let Some((key, _rest)) = line.split_once(':') {
                    let key = key.trim();
                    if !key.is_empty() {
                        keys.insert(key.to_string());
                    }
                }
            } else if !raw.trim().is_empty() {
                in_briefs = false;
            }
        }
    }
    (name, keys)
}

fn declarative_missing_set(findings: &[Diagnostic]) -> BTreeSet<(String, String, String)> {
    let mut out = BTreeSet::new();
    for finding in findings {
        let FindingEvidence::Structured { data, .. } = &finding.evidence else { continue };
        let adapter = data.get("adapter").and_then(|v| v.as_str()).map(str::to_string);
        let axis = data.get("axis").and_then(|v| v.as_str()).map(str::to_string);
        let missing = data.get("missing").and_then(|v| v.as_str()).map(str::to_string);
        if let (Some(a), Some(x), Some(m)) = (adapter, axis, missing) {
            out.insert((a, x, m));
        }
    }
    out
}

#[test]
fn matches_set_coverage() {
    let project = tempfile::tempdir().expect("tempdir");
    let project_dir = project.path();
    stage_project(project_dir);

    let imperative = imperative_missing_set(project_dir);
    let expected: BTreeSet<(String, String, String)> = [
        ("bad-source".to_string(), "sources".to_string(), "extract".to_string()),
        ("bad-target".to_string(), "targets".to_string(), "merge".to_string()),
    ]
    .into_iter()
    .collect();
    assert_eq!(
        imperative, expected,
        "imperative reference must flag exactly the documented (adapter, axis, missing) triples",
    );

    let model = build(project_dir, ScanProfile::Framework, &[], &[]).expect("framework build");
    let rule = make_rule(
        "CORE-004",
        vec![
            hint(HintKind::PathPattern, "adapters/sources/*/adapter.yaml"),
            hint(HintKind::PathPattern, "adapters/targets/*/adapter.yaml"),
            hint(HintKind::SetCoverage, "adapter-briefs-cover-operations"),
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
            Some("CORE-004"),
            "declarative findings must carry the documented CORE-004 rule id",
        );
        let loc = finding.location.as_ref().expect("location set");
        assert!(
            loc.path.starts_with("adapters/"),
            "declarative location must point at an adapter manifest: got {}",
            loc.path,
        );
    }

    let declarative = declarative_missing_set(&outcome.findings);
    assert_eq!(
        declarative, imperative,
        "declarative CORE-004 must flag the same (adapter, axis, missing) triples as the inline set-coverage reference",
    );
}
