//! C8 parity test: prove `CORE-001` covers the retiring `adapter.schema-violation`
//! imperative predicate row.
//!
//! # Equivalence mapping
//!
//! - Imperative rule id `adapter.schema-violation` ↔ declarative rule id `CORE-001`.
//! - Imperative location: absolute `Path` to the offending `adapter.yaml`.
//! - Declarative location: project-relative path string (`adapters/sources/<name>/adapter.yaml`).
//! - Imperative emitted one [`specify_lints::Diagnostic`] per
//!   `jsonschema::Validator::iter_errors` entry; the declarative `schema` hint
//!   evaluator does the same. Both walk the same `iter_errors` iterator over
//!   the same `serde_saphyr`-parsed body, so the cardinality and
//!   `instance_path` set are byte-identical for any given fixture.
//!
//! Because the rule-id field differs, the fingerprint-based
//! deduplication CANNOT silently merge a declarative finding with the
//! retired imperative one during any future overlap window — every parity
//! claim has to be characterised by the (location, JSON-pointer) pair.
//!
//! # Option
//!
//! Option A (functional parity). The test stages a fixture matching the
//! retiring predicate's existing golden (`bad-source` missing
//! `description` + `briefs`; `good-source` complete), then runs:
//!
//! 1. The shared `adapter.schema.json` validator directly via
//!    `jsonschema::validator_for`, mirroring the deleted
//!    `crates/lints/src/framework/check/adapter.rs::validate_manifest` body.
//!    Captures the `instance_path` set per fixture file.
//! 2. The declarative pipeline: `lint::index::build` under the framework
//!    scan profile, plus `lint::eval::evaluate` against a synthesised
//!    `CORE-001` rule carrying the same two hints CORE-001 ships on disk
//!    (`path-pattern: adapters/**/adapter.yaml` +
//!    `schema: ./.cursor/schemas/adapter.schema.json`).
//!
//! Both passes MUST agree on which files violate and on the set of
//! `iter_errors` `instance_path` strings cited per file.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value as JsonValue;
use specify_lints::lint::ScanProfile;
use specify_lints::lint::eval::{ToolOutput, ToolRunError, ToolRunner, evaluate};
use specify_lints::lint::index::build;
use specify_lints::rules::{
    DeterministicHint, Diagnostic, FindingEvidence, HintKind, Origin, PathRoot, ResolvedRule,
    Severity,
};

const BAD_MANIFEST: &str = "name: bad-source\nversion: 1\naxis: source\n";
const GOOD_MANIFEST: &str = concat!(
    "name: good-source\n",
    "version: 1\n",
    "axis: source\n",
    "description: Valid fixture.\n",
    "briefs:\n",
    "  survey: briefs/survey.md\n",
    "  extract: briefs/extract.md\n",
);

fn cli_schemas_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("schemas")
}

fn stage_project(project_dir: &Path) {
    fs::create_dir_all(project_dir.join("plugins")).expect("plugins");
    fs::create_dir_all(project_dir.join("adapters/sources")).expect("sources");
    fs::create_dir_all(project_dir.join("adapters/targets")).expect("targets");

    // Mirror the canonical CLI repo schema into the project tree so
    // CORE-001's `./.cursor/schemas/adapter.schema.json` ref resolves;
    // the framework repo carries the same mirror at the same path
    // (C7 placement). The parity test exercises both validators against
    // the same on-disk bytes.
    let cursor_schemas = project_dir.join(".cursor/schemas");
    fs::create_dir_all(&cursor_schemas).expect("cursor schemas");
    let schema_src = cli_schemas_dir().join("adapter.schema.json");
    fs::copy(&schema_src, cursor_schemas.join("adapter.schema.json")).expect("copy schema");

    let bad_dir = project_dir.join("adapters/sources/bad-source");
    fs::create_dir_all(&bad_dir).expect("bad dir");
    fs::write(bad_dir.join("adapter.yaml"), BAD_MANIFEST).expect("write bad");

    let good_dir = project_dir.join("adapters/sources/good-source");
    fs::create_dir_all(&good_dir).expect("good dir");
    fs::write(good_dir.join("adapter.yaml"), GOOD_MANIFEST).expect("write good");
}

/// Reproduces the deleted imperative
/// `check::adapter::{load_runtime_validator, validate_manifest}` body
/// inline so the parity claim is anchored to executable code in this
/// commit. Mirrors the validator setup, the YAML parser, and the per-error
/// `instance_path` capture verbatim — the deleted message-formatting layer
/// only affected the human-readable string, not the violation set itself.
fn imperative_pointer_set(project_dir: &Path, manifest_rel: &str) -> BTreeSet<String> {
    let schema_path = cli_schemas_dir().join("adapter.schema.json");
    let schema_body = fs::read_to_string(&schema_path).expect("read schema");
    let schema_json: JsonValue = serde_json::from_str(&schema_body).expect("schema json");
    let validator = jsonschema::validator_for(&schema_json).expect("schema compiles");

    let manifest_body = fs::read_to_string(project_dir.join(manifest_rel)).expect("read manifest");
    let instance: JsonValue = serde_saphyr::from_str(&manifest_body).expect("manifest yaml parse");
    validator.iter_errors(&instance).map(|err| err.instance_path().to_string()).collect()
}

fn declarative_pointer_set(findings: &[Diagnostic], manifest_rel: &str) -> BTreeSet<String> {
    findings
        .iter()
        .filter(|f| f.location.as_ref().is_some_and(|loc| loc.path == manifest_rel))
        .filter_map(|f| match &f.evidence {
            FindingEvidence::Structured { data, .. } => {
                data.get("json_pointer").and_then(JsonValue::as_str).map(str::to_owned)
            }
            _ => None,
        })
        .collect()
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
fn core_001_matches_imperative_schema_row() {
    let project = tempfile::tempdir().expect("tempdir");
    let project_dir = project.path();
    stage_project(project_dir);

    let bad_rel = "adapters/sources/bad-source/adapter.yaml";
    let good_rel = "adapters/sources/good-source/adapter.yaml";

    let imperative_bad = imperative_pointer_set(project_dir, bad_rel);
    let imperative_good = imperative_pointer_set(project_dir, good_rel);
    assert!(
        !imperative_bad.is_empty(),
        "imperative row must flag the bad manifest (parity fixture invariant)"
    );
    assert!(
        imperative_good.is_empty(),
        "imperative row must not flag the good manifest: {imperative_good:?}"
    );

    let model = build(project_dir, ScanProfile::Framework, &[], &[]).expect("framework build");
    let rule = make_rule(
        "CORE-001",
        vec![
            hint(HintKind::PathPattern, "adapters/**/adapter.yaml"),
            hint(HintKind::Schema, "./.cursor/schemas/adapter.schema.json"),
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
            Some("CORE-001"),
            "declarative findings must carry the documented CORE-001 rule id",
        );
    }

    let declarative_bad = declarative_pointer_set(&outcome.findings, bad_rel);
    let declarative_good = declarative_pointer_set(&outcome.findings, good_rel);

    assert_eq!(
        declarative_bad, imperative_bad,
        "declarative CORE-001 must cite the same instance pointers on the bad manifest as the retired adapter.schema-violation predicate",
    );
    assert!(
        declarative_good.is_empty(),
        "declarative CORE-001 must not flag the good manifest: {declarative_good:?}"
    );
}
