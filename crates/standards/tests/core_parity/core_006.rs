//! `CORE-006` ≅ the `constant-eq` reserved-kind semantics: adapter manifest
//! `version:` must equal `"1"`. No imperative `Check` row is retired; an
//! inline reference stands in.

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
    "version: 2\n",
    "axis: source\n",
    "description: Source adapter declaring the wrong manifest version.\n",
    "briefs:\n",
    "  survey: briefs/survey.md\n",
    "  extract: briefs/extract.md\n",
);
const BAD_TARGET: &str = concat!(
    "name: bad-target\n",
    "version: \"0.9\"\n",
    "axis: target\n",
    "description: Target adapter declaring a pre-v1 manifest version.\n",
    "briefs:\n",
    "  shape: briefs/shape.md\n",
    "  build: briefs/build.md\n",
    "  merge: briefs/merge.md\n",
);
const GOOD_SOURCE: &str = concat!(
    "name: good-source\n",
    "version: 1\n",
    "axis: source\n",
    "description: Conforming source adapter (negative control).\n",
    "briefs:\n",
    "  survey: briefs/survey.md\n",
    "  extract: briefs/extract.md\n",
);

const EXPECTED_VERSION: &str = "1";

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

/// Inline reference mirroring `kind: constant-eq`; returns the
/// `(adapter, axis, actual_version)` triple set for manifests whose
/// `version:` is not `EXPECTED_VERSION`.
fn imperative_mismatch_set(project_dir: &Path) -> BTreeSet<(String, String, String)> {
    let mut out = BTreeSet::new();
    for axis in ["sources", "targets"] {
        let axis_dir = project_dir.join("adapters").join(axis);
        let Ok(entries) = fs::read_dir(&axis_dir) else { continue };
        for entry in entries.flatten() {
            let dir = entry.path();
            if !dir.is_dir() {
                continue;
            }
            let manifest_path = dir.join("adapter.yaml");
            let Ok(body) = fs::read_to_string(&manifest_path) else { continue };
            let (name, version) = parse_manifest(&body);
            let Some(name) = name else { continue };
            let actual = version.unwrap_or_else(|| "(absent)".to_string());
            if actual != EXPECTED_VERSION {
                out.insert((name, axis.to_string(), actual));
            }
        }
    }
    out
}

/// Minimal manifest parser: the `name:` and `version:` top-level scalars.
fn parse_manifest(body: &str) -> (Option<String>, Option<String>) {
    let mut name: Option<String> = None;
    let mut version: Option<String> = None;
    for raw in body.lines() {
        if let Some(stripped) = raw.strip_prefix("name:") {
            let trimmed = stripped.trim().trim_matches(|c: char| c == '"' || c == '\'');
            if !trimmed.is_empty() {
                name = Some(trimmed.to_string());
            }
            continue;
        }
        if let Some(stripped) = raw.strip_prefix("version:") {
            let trimmed = stripped.trim().trim_matches(|c: char| c == '"' || c == '\'');
            if !trimmed.is_empty() {
                version = Some(trimmed.to_string());
            }
        }
    }
    (name, version)
}

fn declarative_mismatch_set(findings: &[Diagnostic]) -> BTreeSet<(String, String, String)> {
    let mut out = BTreeSet::new();
    for finding in findings {
        let FindingEvidence::Structured { data, .. } = &finding.evidence else { continue };
        let adapter = data.get("adapter").and_then(|v| v.as_str()).map(str::to_string);
        let actual = data.get("actual").and_then(|v| v.as_str()).map(str::to_string);
        let path = data.get("path").and_then(|v| v.as_str()).unwrap_or("");
        let axis = axis_from_path(path).map(str::to_string);
        if let (Some(a), Some(x), Some(v)) = (adapter, axis, actual) {
            out.insert((a, x, v));
        }
    }
    out
}

/// Recover the `sources` / `targets` axis from a manifest's project-relative path.
fn axis_from_path(path: &str) -> Option<&'static str> {
    let rest = path.strip_prefix("adapters/")?;
    let (axis, _rest) = rest.split_once('/')?;
    match axis {
        "sources" => Some("sources"),
        "targets" => Some("targets"),
        _ => None,
    }
}

#[test]
fn matches_constant_eq() {
    let project = tempfile::tempdir().expect("tempdir");
    let project_dir = project.path();
    stage_project(project_dir);

    let imperative = imperative_mismatch_set(project_dir);
    let expected: BTreeSet<(String, String, String)> = [
        ("bad-source".to_string(), "sources".to_string(), "2".to_string()),
        ("bad-target".to_string(), "targets".to_string(), "0.9".to_string()),
    ]
    .into_iter()
    .collect();
    assert_eq!(
        imperative, expected,
        "imperative reference must flag exactly the documented (adapter, axis, actual) triples",
    );

    let model = build(project_dir, ScanProfile::Framework, &[], &[]).expect("framework build");
    let rule = make_rule(
        "CORE-006",
        vec![
            hint(HintKind::PathPattern, "adapters/sources/*/adapter.yaml"),
            hint(HintKind::PathPattern, "adapters/targets/*/adapter.yaml"),
            hint(HintKind::ConstantEq, "adapter-manifest-version-equals-v1"),
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
            Some("CORE-006"),
            "declarative findings must carry the documented CORE-006 rule id",
        );
        let loc = finding.location.as_ref().expect("location set");
        assert!(
            loc.path.starts_with("adapters/"),
            "declarative location must point at an adapter manifest: got {}",
            loc.path,
        );
    }

    let declarative = declarative_mismatch_set(&outcome.findings);
    assert_eq!(
        declarative, imperative,
        "declarative CORE-006 must flag the same (adapter, axis, actual) triples as the inline constant-eq reference",
    );
}
