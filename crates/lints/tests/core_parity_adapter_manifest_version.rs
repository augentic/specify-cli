//! C14 parity test: prove `CORE-006` covers the `constant-eq`
//! reserved-kind semantics for adapter manifest `version:` against
//! the literal `"1"`.
//!
//! # Equivalence mapping
//!
//! - Declarative rule id `CORE-006` ≅ `adapter.manifest-version`
//!   (notional; no imperative `Check` row currently enforces the
//!   `version: 1` invariant). No imperative deletion is in flight:
//!   the fingerprint dedup contract's fallback explicitly permits
//!   landing the kind interpreter + seed rule against a synthetic
//!   fixture when no `Check` row maps cleanly to the reserved kind
//!   ("the imperative deletion is gated on parity, not required for
//!   the kind landing"). The closest existing imperative surface,
//!   `crates/lints/src/framework/check/tools.rs::FirstPartyTools`, asserts
//!   adapter-specific tool-package constants
//!   (`specify:contract@0.3.0`, `specify:vectis@0.3.0`) but mixes
//!   existence + equality across multiple `(adapter, tool, package)`
//!   triples and is not cleanly modelled by a single
//!   constant-equality discriminator. It is therefore explicitly NOT
//!   retired by this card.
//! - Imperative behaviour (anchored as executable code in this test
//!   crate): walk every `adapters/{sources,targets}/<name>/adapter.yaml`,
//!   parse the `name:` scalar and the `version:` scalar, return the
//!   `(adapter, axis, actual_version)` triple set for every manifest
//!   whose `version:` does not equal the literal `"1"`.
//! - Declarative behaviour: the framework-profile indexer extracts
//!   one [`specify_lints::lint::AdapterManifest`] fact per well-formed
//!   manifest (`crates/lints/src/lint/index/adapter.rs::extract`),
//!   whose `version` field stringifies both integer (`1`) and string
//!   (`"2.1"`) YAML forms; the `kind: constant-eq` interpreter
//!   (`crates/lints/src/lint/eval/constant_eq.rs::evaluate`)
//!   consumes the fact set and emits one [`Diagnostic`] per
//!   manifest whose `version` is not the string `"1"`, carrying the
//!   `(adapter, path, field, actual, expected)` shape as structured
//!   evidence.
//!
//! Because no imperative deletion is in flight, the fingerprint-based
//! deduplication has nothing to merge in any overlap
//! window.
//!
//! # Option
//!
//! Option A (functional parity) against a synthetic fixture. The
//! test stages three adapter manifests:
//!
//! - `bad-source` — `version: 2` (string `"2"` after stringification);
//!   expected to be flagged by both passes.
//! - `bad-target` — `version: "0.9"`; expected to be flagged by both
//!   passes.
//! - `good-source` — `version: 1`; negative control.
//!
//! Then runs:
//!
//! 1. An inline reference implementation that mirrors the
//!    `kind: constant-eq` semantics (parse `name:` + `version:`,
//!    return the `(adapter, axis, actual_version)` triple set for
//!    every manifest whose `version:` does not equal `"1"`). This
//!    stands in for the "imperative row" anchored as executable code
//!    in this test crate so the parity claim is not purely
//!    tautological.
//! 2. The declarative pipeline: `lint::index::build` under the
//!    framework scan profile (which populates `model.adapter_manifests`
//!    with the `version` field), then `lint::eval::evaluate` against
//!    a synthesised `CORE-006` rule carrying the same three hints
//!    CORE-006 ships on disk (two `path-pattern`s — one per axis —
//!    plus `constant-eq: adapter-manifest-version-equals-v1`).
//!
//! Both passes MUST agree on the `(adapter, axis, actual_version)`
//! triple set. Per-finding locations are NOT compared byte-identically
//! because the imperative reference returns an abstract triple while
//! the declarative evaluator stamps a project-relative path location;
//! functional parity (which adapters were flagged with which actual
//! version) is the contract.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use specify_diagnostics::{Diagnostic, FindingEvidence, Severity};
use specify_lints::lint::ScanProfile;
use specify_lints::lint::eval::{ToolOutput, ToolRunError, ToolRunner, evaluate};
use specify_lints::lint::index::build;
use specify_lints::rules::{DeterministicHint, HintKind, Origin, PathRoot, ResolvedRule};

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

/// Inline reference implementation mirroring the
/// `kind: constant-eq` semantics so the parity claim is anchored to
/// executable code in this commit. Walks every manifest under
/// `adapters/{sources,targets}/<name>/adapter.yaml`, parses the
/// `name:` and `version:` scalars, and returns the
/// `(adapter, axis, actual_version)` triple set per manifests whose
/// `version:` does not equal `EXPECTED_VERSION`.
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

/// Minimal manifest parser: extract the `name:` and `version:`
/// top-level scalars. Sufficient for the parity fixture because the
/// synthesised manifests are well-formed; integer and quoted-string
/// scalar forms are both flattened to the stringified value the
/// declarative extractor produces.
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
fn core_006_matches_constant_eq_reference_against_adapter_manifest_version() {
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
