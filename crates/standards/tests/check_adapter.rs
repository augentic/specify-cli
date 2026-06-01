//! Integration coverage for the framework adapter-manifest check.

use std::fs;
use std::path::Path;

use specify_standards::framework::check::{RULE_MISSING_MANIFEST, run_adapter_check};
use specify_standards::framework::{Context, core_id_for, snippet};

fn scaffold_framework(root: &Path) {
    fs::create_dir_all(root.join("plugins")).expect("plugins");
    fs::create_dir_all(root.join("adapters/sources")).expect("sources");
    fs::create_dir_all(root.join("adapters/targets")).expect("targets");
}

// Schema-violation coverage moved to the declarative pipeline; see
// `core_parity_adapter_schema::core_001_matches_imperative_schema_row`
// for the equivalence proof against the retired
// `adapter.schema-violation` predicate row.

#[test]
fn missing_manifest_without_yaml() {
    let temp = tempfile::tempdir().expect("tempdir");
    scaffold_framework(temp.path());

    fs::create_dir_all(temp.path().join("adapters/sources/no-manifest")).expect("adapter dir");

    let ctx = Context::from_framework_root(temp.path()).expect("context");
    let findings = run_adapter_check(&ctx);

    let missing: Vec<_> = findings
        .iter()
        .filter(|finding| finding.rule_id.as_deref() == core_id_for(RULE_MISSING_MANIFEST))
        .collect();
    assert_eq!(missing.len(), 1, "expected one missing-manifest finding");
    assert!(snippet(missing[0]).contains("adapters/sources/no-manifest"));
    assert!(snippet(missing[0]).contains("adapter.yaml"));
}
