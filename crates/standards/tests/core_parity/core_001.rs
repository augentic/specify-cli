//! `CORE-001` ≅ the retiring `adapter.schema-violation` imperative row.
//! Both walk the same `iter_errors` set over the same `serde_saphyr`-parsed
//! `adapter.yaml`, so the `instance_path` pointer set must be byte-identical.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value as JsonValue;
use specify_diagnostics::{Diagnostic, FindingEvidence};
use specify_standards::lint::ScanProfile;
use specify_standards::lint::eval::{ToolRunner, evaluate};
use specify_standards::lint::index::build;
use specify_standards::rules::HintKind;

use crate::eval_support::{NoToolRunner, hint, make_rule};

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
/// commit.
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

#[test]
fn matches_imperative_schema_row() {
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
