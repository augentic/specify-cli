//! `CORE-007` ≅ the `set-eq` reserved-kind semantics: adapter manifest
//! `briefs.keys()` must exactly equal the axis-appropriate operation enum
//! (both missing and unexpected keys). No imperative `Check` row is retired.

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
    "description: Target adapter missing `merge:` and carrying an unexpected key.\n",
    "briefs:\n",
    "  shape: briefs/shape.md\n",
    "  build: briefs/build.md\n",
    "  extra: briefs/extra.md\n",
);
const EXTRA_SOURCE: &str = concat!(
    "name: extra-source\n",
    "version: 1\n",
    "axis: source\n",
    "description: Source adapter with a complete set plus a stray key.\n",
    "briefs:\n",
    "  survey: briefs/survey.md\n",
    "  extract: briefs/extract.md\n",
    "  legacy: briefs/legacy.md\n",
);
const GOOD_TARGET: &str = concat!(
    "name: good-target\n",
    "version: 1\n",
    "axis: target\n",
    "description: Exactly the target operation set (negative control).\n",
    "briefs:\n",
    "  shape: briefs/shape.md\n",
    "  build: briefs/build.md\n",
    "  merge: briefs/merge.md\n",
);

const SOURCE_OPERATIONS: &[&str] = &["extract", "survey"];
const TARGET_OPERATIONS: &[&str] = &["build", "merge", "shape"];

const DIVERGENCE_MISSING: &str = "missing";
const DIVERGENCE_UNEXPECTED: &str = "unexpected";

fn stage_project(project_dir: &Path) {
    fs::create_dir_all(project_dir.join("plugins")).expect("plugins");
    fs::create_dir_all(project_dir.join("adapters/sources")).expect("sources");
    fs::create_dir_all(project_dir.join("adapters/targets")).expect("targets");

    for (rel, body) in [
        ("adapters/sources/bad-source/adapter.yaml", BAD_SOURCE),
        ("adapters/targets/bad-target/adapter.yaml", BAD_TARGET),
        ("adapters/sources/extra-source/adapter.yaml", EXTRA_SOURCE),
        ("adapters/targets/good-target/adapter.yaml", GOOD_TARGET),
    ] {
        let path = project_dir.join(rel);
        fs::create_dir_all(path.parent().expect("parent")).expect("manifest dir");
        fs::write(&path, body).expect("write manifest");
    }
}

/// Inline reference mirroring `kind: set-eq`; returns the
/// `(adapter, axis, divergence, operation)` quadruple set for both halves
/// of the symmetric difference against the axis-appropriate enum.
fn imperative_divergence_set(project_dir: &Path) -> BTreeSet<(String, String, String, String)> {
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
                    out.insert((
                        name.clone(),
                        axis.to_string(),
                        DIVERGENCE_MISSING.to_string(),
                        (*op).to_string(),
                    ));
                }
            }
            for key in &keys {
                if !expected.contains(key.as_str()) {
                    out.insert((
                        name.clone(),
                        axis.to_string(),
                        DIVERGENCE_UNEXPECTED.to_string(),
                        key.clone(),
                    ));
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

fn declarative_divergence_set(
    findings: &[Diagnostic],
) -> BTreeSet<(String, String, String, String)> {
    let mut out = BTreeSet::new();
    for finding in findings {
        let FindingEvidence::Structured { data, .. } = &finding.evidence else { continue };
        let adapter = data.get("adapter").and_then(|v| v.as_str()).map(str::to_string);
        let axis = data.get("axis").and_then(|v| v.as_str()).map(str::to_string);
        let divergence = data.get("divergence").and_then(|v| v.as_str()).map(str::to_string);
        let operation = data.get("operation").and_then(|v| v.as_str()).map(str::to_string);
        if let (Some(a), Some(x), Some(d), Some(o)) = (adapter, axis, divergence, operation) {
            out.insert((a, x, d, o));
        }
    }
    out
}

#[test]
fn matches_set_eq() {
    let project = tempfile::tempdir().expect("tempdir");
    let project_dir = project.path();
    stage_project(project_dir);

    let imperative = imperative_divergence_set(project_dir);
    let expected: BTreeSet<(String, String, String, String)> = [
        ("bad-source", "sources", DIVERGENCE_MISSING, "extract"),
        ("bad-target", "targets", DIVERGENCE_MISSING, "merge"),
        ("bad-target", "targets", DIVERGENCE_UNEXPECTED, "extra"),
        ("extra-source", "sources", DIVERGENCE_UNEXPECTED, "legacy"),
    ]
    .into_iter()
    .map(|(a, x, d, o)| (a.to_string(), x.to_string(), d.to_string(), o.to_string()))
    .collect();
    assert_eq!(
        imperative, expected,
        "imperative reference must flag exactly the documented (adapter, axis, divergence, operation) quadruples",
    );

    let model = build(project_dir, ScanProfile::Framework, &[], &[]).expect("framework build");
    let rule = make_rule(
        "CORE-007",
        vec![
            hint(HintKind::PathPattern, "adapters/sources/*/adapter.yaml"),
            hint(HintKind::PathPattern, "adapters/targets/*/adapter.yaml"),
            hint(HintKind::SetEq, "adapter-briefs-equal-operations"),
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
            Some("CORE-007"),
            "declarative findings must carry the documented CORE-007 rule id",
        );
        let loc = finding.location.as_ref().expect("location set");
        assert!(
            loc.path.starts_with("adapters/"),
            "declarative location must point at an adapter manifest: got {}",
            loc.path,
        );
    }

    let declarative = declarative_divergence_set(&outcome.findings);
    assert_eq!(
        declarative, imperative,
        "declarative CORE-007 must flag the same (adapter, axis, divergence, operation) quadruples as the inline set-eq reference",
    );
}
