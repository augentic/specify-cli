//! C15 parity test: prove `CORE-007` covers the `set-eq` reserved-kind
//! semantics for adapter manifest `briefs.keys()` against the closed
//! axis-appropriate operation enums, on BOTH sides (missing operations
//! and unexpected keys).
//!
//! # Equivalence mapping
//!
//! - Declarative rule id `CORE-007` ≅ `adapter.briefs-equal-operations`
//!   (notional). No imperative `Check` row is retired by this card.
//!   The brief-completeness surface the C15 plan card named as a strong
//!   candidate (`briefs.keys() == operations_for_axis(axis)` in
//!   `crates/lints/src/framework/check/adapter.rs`) does not exist as a clean
//!   imperative row: that module retains only `adapter.missing-manifest`,
//!   which walks adapter directories lacking `adapter.yaml` entirely and
//!   (per its own doc comment) produces no `AdapterManifest` fact, so it
//!   is invisible to a fact-iterating `set-eq` evaluator. The two halves
//!   of the brief-key invariant are already owned declaratively:
//!   `CORE-001` ≅ `adapter.schema` covers `required` (missing keys) and
//!   `additionalProperties: false` (unknown keys) via its `schema` hint,
//!   and `CORE-004` ≅ `adapter.briefs-cover-operations` covers the
//!   missing half via its one-sided `set-coverage` hint. `CORE-007`
//!   tightens to strict equality so the `unexpected`-key half is
//!   attributed to a specific operation rather than the generic schema
//!   envelope. The migration-cadence fallback explicitly permits landing
//!   the kind interpreter + seed rule against a synthetic fixture when no
//!   `Check` row maps cleanly (the imperative deletion is gated on
//!   parity, not required for the kind landing).
//! - Imperative behaviour (anchored as executable code in this test
//!   crate): walk every `adapters/{sources,targets}/<name>/adapter.yaml`,
//!   parse the `briefs:` map keys, and return the
//!   `(adapter, axis, divergence, operation)` quadruple set, where
//!   `divergence` is `missing` for an operation in the closed
//!   axis-appropriate enum absent from `briefs.keys()`, and `unexpected`
//!   for a `briefs.keys()` entry absent from the enum
//!   (`SourceOperation` ≡ `{survey, extract}`,
//!   `TargetOperation` ≡ `{shape, build, merge}`).
//! - Declarative behaviour: the framework-profile indexer extracts one
//!   [`specify_lints::lint::AdapterManifest`] fact per well-formed
//!   manifest (`crates/lints/src/lint/index/adapter.rs::extract`,
//!   whose `brief_keys` field mirrors the `briefs:` map keys verbatim);
//!   the `kind: set-eq` interpreter
//!   (`crates/lints/src/lint/eval/set_eq.rs::evaluate`) consumes
//!   the fact set and emits one [`Diagnostic`] per `(adapter, divergence)`
//!   pair, carrying the `(adapter, axis, divergence, operation, expected,
//!   actual)` shape as structured evidence.
//!
//! Because no imperative deletion is in flight, the fingerprint-based
//! deduplication in the migration cadence has nothing to merge in any
//! overlap window; `CORE-001` / `CORE-004` / `CORE-007` findings land
//! side-by-side when they overlap and dedupe cleanly through the
//! fingerprint algorithm on identical `(rule-id, location, evidence)`
//! triples.
//!
//! # Option
//!
//! Option A (functional parity) against a synthetic fixture. The test
//! stages four adapter manifests exercising both divergence directions:
//!
//! - `bad-source` — declares `survey:` only; `extract:` is `missing`.
//! - `bad-target` — declares `shape:` + `build:` + an `extra:` key; the
//!   `extra:` key is `unexpected` and `merge:` is `missing`.
//! - `extra-source` — declares the full `{survey, extract}` set plus a
//!   stray `legacy:` key; the `legacy:` key is `unexpected` (no missing).
//! - `good-target` — declares the exact `{shape, build, merge}` set;
//!   negative control.
//!
//! Then runs:
//!
//! 1. An inline reference implementation that mirrors the `kind: set-eq`
//!    semantics (parse `briefs:` map keys, derive the axis-appropriate
//!    expected set from the source / target operation enums, return the
//!    `(adapter, axis, divergence, operation)` quadruple set for both
//!    halves of the symmetric difference). This stands in for the
//!    "imperative row" anchored as executable code in this test crate so
//!    the parity claim is not purely tautological.
//! 2. The declarative pipeline: `lint::index::build` under the framework
//!    scan profile (which populates `model.adapter_manifests` with the
//!    `brief_keys` field), then `lint::eval::evaluate` against a
//!    synthesised `CORE-007` rule carrying the same three hints CORE-007
//!    ships on disk (two `path-pattern`s — one per axis — plus
//!    `set-eq: adapter-briefs-equal-operations`).
//!
//! Both passes MUST agree on the `(adapter, axis, divergence, operation)`
//! quadruple set. Per-finding locations are NOT compared byte-identically
//! because the imperative reference returns an abstract quadruple while
//! the declarative evaluator stamps a project-relative path location;
//! functional parity (which adapters diverged, in which direction, on
//! which operation) is the contract.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use specify_lints::lint::ScanProfile;
use specify_lints::lint::eval::{ToolOutput, ToolRunError, ToolRunner, evaluate};
use specify_lints::lint::index::build;
use specify_lints::rules::{
    DeterministicHint, Diagnostic, FindingEvidence, HintKind, Origin, PathRoot, ResolvedRule,
    Severity,
};

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

/// Inline reference implementation mirroring the `kind: set-eq`
/// semantics so the parity claim is anchored to executable code in this
/// commit. Walks every manifest under
/// `adapters/{sources,targets}/<name>/adapter.yaml`, parses the
/// `briefs:` map keys, and returns the
/// `(adapter, axis, divergence, operation)` quadruple set per the
/// symmetric difference against the closed axis-appropriate operation
/// enum (`SourceOperation` ≡ `{survey, extract}`,
/// `TargetOperation` ≡ `{shape, build, merge}`).
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

/// Minimal manifest parser: extract the `name:` scalar and the keys of
/// the top-level `briefs:` map. Sufficient for the parity fixture
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
fn core_007_matches_set_eq_reference_against_adapter_briefs() {
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
