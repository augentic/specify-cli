//! C12 parity test: prove `CORE-004` covers the `set-coverage` reserved-kind
//! semantics for adapter manifest `briefs.keys()` against the closed
//! axis-appropriate operation enums.
//!
//! # Equivalence mapping
//!
//! - Declarative rule id `CORE-004` ≅ `adapter.briefs-cover-operations`.
//!   No imperative `Check` row is retired by this card: the JSON schema in
//!   `source.schema.json` / `target.schema.json` already requires the full
//!   `briefs.{survey,extract}` / `briefs.{shape,build,merge}` key set via
//!   `required: […]`, and that surface is owned end-to-end by the
//!   `CORE-001` ≅ `adapter.schema-violation` migration. The
//!   fingerprint dedup contract explicitly permits landing the kind interpreter +
//!   seed rule against a synthetic fixture when no `Check` row maps
//!   cleanly to the reserved kind (the deletion is gated on parity, not
//!   required for the kind landing).
//! - `adapter.missing-manifest` (in `crates/standards/src/framework/check/adapter.rs`)
//!   was considered but explicitly NOT retired: it walks adapter directories
//!   that lack `adapter.yaml` entirely, which produce no `AdapterManifest`
//!   fact and are therefore invisible to a `set-coverage` evaluator that
//!   iterates `WorkspaceModel.adapter_manifests`. The closer declarative
//!   fit for directory existence is a future `cardinality` / `set-eq`
//!   rule.
//!
//! Because no imperative deletion is in flight, the fingerprint-based
//! deduplication has nothing to merge in any overlap
//! window; CORE-001's schema-violation finding and CORE-004's
//! per-operation finding land side-by-side when both fire and dedupe
//! cleanly through the existing fingerprint algorithm on identical
//! `(rule-id, location, evidence)` triples.
//!
//! # Option
//!
//! Option A (functional parity) against a synthetic fixture: the test
//! stages one source-adapter manifest missing the `extract:` brief key
//! and one target-adapter manifest missing the `merge:` brief key, plus
//! one fully-covered source manifest as a negative control, then runs:
//!
//! 1. An inline reference implementation that mirrors the
//!    `kind: set-coverage` semantics (parse `briefs:` map, derive the
//!    axis-appropriate expected set from the source / target operation
//!    enums, return the `(adapter, axis, missing-operation)` triple
//!    set). This stands in for the "imperative row" anchored as
//!    executable code in this test crate so the parity claim is not
//!    purely tautological.
//! 2. The declarative pipeline: `lint::index::build` under the framework
//!    scan profile (which populates `model.adapter_manifests` with the
//!    new `brief_keys` field), then `lint::eval::evaluate` against a
//!    synthesised `CORE-004` rule carrying the same three hints CORE-004
//!    ships on disk (two `path-pattern`s — one per axis — plus
//!    `set-coverage: adapter-briefs-cover-operations`).
//!
//! Both passes MUST agree on the `(adapter, axis, missing-operation)`
//! triple set. Per-finding locations are NOT compared byte-identically
//! because the imperative reference returns an abstract triple while
//! the declarative evaluator stamps a project-relative path location;
//! functional parity (which adapters were flagged with which missing
//! operations) is the contract.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use specify_diagnostics::{Diagnostic, FindingEvidence, Severity};
use specify_standards::lint::ScanProfile;
use specify_standards::lint::eval::{ToolOutput, ToolRunError, ToolRunner, evaluate};
use specify_standards::lint::index::build;
use specify_standards::rules::{DeterministicHint, HintKind, Origin, PathRoot, ResolvedRule};

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

/// Inline reference implementation mirroring the
/// `kind: set-coverage` semantics so the parity claim is anchored to
/// executable code in this commit. Walks every manifest under
/// `adapters/{sources,targets}/<name>/adapter.yaml`, parses the
/// `briefs:` map keys, and returns the
/// `(adapter, axis, missing-operation)` triple set per the closed
/// axis-appropriate operation enums (`SourceOperation` ≡ `{survey, extract}`,
/// `TargetOperation` ≡ `{shape, build, merge}`).
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

/// Minimal manifest parser: extract the `name:` scalar and the keys
/// of the top-level `briefs:` map. Sufficient for the parity fixture
/// because the synthesised manifests are well-formed.
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
fn core_004_matches_set_coverage_reference_against_adapter_briefs() {
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
