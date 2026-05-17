//! Integration tests for the `--sources` YAML parser
//! (`SourcesFile::parse`).

use specify_domain::survey::SourcesFile;

#[test]
fn valid_sources_parses() {
    let yaml = "\
version: 1
sources:
  - key: legacy-monolith
    path: ./legacy/monolith
  - key: legacy-billing
    path: ./legacy/billing
";
    let file = SourcesFile::parse(yaml).unwrap();
    assert_eq!(file.version, 1);
    assert_eq!(file.sources.len(), 2);
    assert_eq!(file.sources[0].key, "legacy-monolith");
    assert_eq!(file.sources[1].key, "legacy-billing");
}

#[test]
fn missing_version_is_malformed() {
    let yaml = "\
sources:
  - key: a
    path: ./a
";
    let err = SourcesFile::parse(yaml).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("malformed") || msg.contains("missing"), "expected malformed, got: {msg}");
}

#[test]
fn bad_version_is_malformed() {
    let yaml = "\
version: 2
sources:
  - key: a
    path: ./a
";
    let err = SourcesFile::parse(yaml).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("unsupported version"), "expected version error, got: {msg}");
}

#[test]
fn empty_sources_is_malformed() {
    let yaml = "\
version: 1
sources: []
";
    let err = SourcesFile::parse(yaml).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("empty"), "expected empty error, got: {msg}");
}

#[test]
fn duplicate_key_is_malformed() {
    let yaml = "\
version: 1
sources:
  - key: same
    path: ./a
  - key: same
    path: ./b
";
    let err = SourcesFile::parse(yaml).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("duplicate key"), "expected duplicate key error, got: {msg}");
}

#[test]
fn not_yaml_is_malformed() {
    let err = SourcesFile::parse("{{not yaml").unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("malformed"), "expected malformed, got: {msg}");
}
