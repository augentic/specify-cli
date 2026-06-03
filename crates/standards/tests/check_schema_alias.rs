//! Integration coverage for the cross-repo rule-schema-alias parity
//! check. `specify lint framework` carries the canonical `rule.schema.json` embedded in
//! the binary and lints a framework root that owns the editor mirror at
//! `.cursor/schemas/rule.schema.json`; this check is the seam that keeps
//! their hint-kind vocabularies in lockstep across the two repos.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde_json::Value;
use specify_standards::framework::check::schema_alias::run_on_root;
use specify_standards::framework::snippet;

/// Pull the canonical hint-kind vocabulary out of the embedded schema so
/// the tests assert against the same source of truth the check uses.
fn canonical_kinds() -> Vec<String> {
    let schema: Value = serde_json::from_str(specify_schema::RULE_JSON_SCHEMA).expect("parses");
    let kind = schema
        .pointer("/properties/rule_hints/items/properties/kind/oneOf")
        .and_then(Value::as_array)
        .expect("canonical kind oneOf");
    kind.iter().filter_map(|b| b.get("const").and_then(Value::as_str)).map(str::to_owned).collect()
}

/// Write a minimal editor-mirror alias whose `kind` enum is `kinds`.
fn write_alias(root: &Path, kinds: &[String]) {
    let body = serde_json::json!({
        "properties": {
            "rule_hints": {
                "items": { "properties": { "kind": { "enum": kinds } } }
            }
        }
    });
    let path = root.join(".cursor/schemas/rule.schema.json");
    fs::create_dir_all(path.parent().unwrap()).expect("schemas dir");
    fs::write(path, serde_json::to_string_pretty(&body).unwrap()).expect("write alias");
}

#[test]
fn matching_vocabulary_passes() {
    let tmp = tempfile::tempdir().expect("tempdir");
    write_alias(tmp.path(), &canonical_kinds());

    let findings = run_on_root(tmp.path());
    assert!(findings.is_empty(), "expected no findings, got: {findings:?}");
}

#[test]
fn absent_alias_is_skipped() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let findings = run_on_root(tmp.path());
    assert!(findings.is_empty(), "roots without the mirror are skipped");
}

#[test]
fn missing_kind_fails() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut kinds = canonical_kinds();
    let dropped = kinds.pop().expect("at least one kind");
    write_alias(tmp.path(), &kinds);

    let findings = run_on_root(tmp.path());
    assert_eq!(findings.len(), 1, "{findings:?}");
    let msg = snippet(&findings[0]);
    assert!(msg.contains(&dropped), "message should name the missing kind: {msg}");
    assert!(msg.contains("missing from the alias"), "{msg}");
}

#[test]
fn extra_kind_fails() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut kinds = canonical_kinds();
    kinds.push("not-a-real-kind".to_owned());
    write_alias(tmp.path(), &kinds);

    let findings = run_on_root(tmp.path());
    assert_eq!(findings.len(), 1, "{findings:?}");
    let msg = snippet(&findings[0]);
    assert!(msg.contains("not-a-real-kind"), "{msg}");
    assert!(msg.contains("unknown to the CLI"), "{msg}");
}

#[test]
fn invalid_json_fails() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join(".cursor/schemas/rule.schema.json");
    fs::create_dir_all(path.parent().unwrap()).expect("schemas dir");
    fs::write(&path, "{ not json").expect("write");

    let findings = run_on_root(tmp.path());
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert!(snippet(&findings[0]).contains("not valid JSON"), "{}", snippet(&findings[0]));
}

#[test]
fn live_specify_alias_matches_canonical() {
    // The sibling specify repo is the real cross-repo seam. When it is
    // resolvable from this workspace (local dev / combined checkout),
    // assert its mirror is in lockstep; skip in single-repo CI.
    let sibling = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../specify");
    if !sibling.join(".cursor/schemas/rule.schema.json").is_file() {
        return;
    }
    let findings = run_on_root(&sibling);
    assert!(findings.is_empty(), "live specify alias drifted: {findings:?}");

    // Sanity: the canonical vocabulary is non-trivial.
    let kinds: BTreeSet<String> = canonical_kinds().into_iter().collect();
    assert!(kinds.len() >= 12, "expected the full hint-kind vocabulary, got {}", kinds.len());
}
