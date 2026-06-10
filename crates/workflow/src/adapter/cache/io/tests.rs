use tempfile::tempdir;

use super::*;
use crate::adapter::cache::{FingerprintSource, FingerprintToolVersion};
use crate::journal::test_timestamp;

fn fp(adapter: &str) -> CacheFingerprint {
    CacheFingerprint::new(
        FingerprintSource::Path {
            path: "/repo/legacy".to_string(),
        },
        adapter.to_string(),
        "sha256:brief".to_string(),
        vec![FingerprintToolVersion {
            name: "tsc".to_string(),
            version: Some("5.4.0".to_string()),
        }],
        Some("user-registration".to_string()),
    )
}

fn index_entry(layout_adapter: &str, fingerprint: &CacheFingerprint) -> CacheIndexEntry {
    CacheIndexEntry {
        timestamp: test_timestamp("2026-05-22T13:15:00Z"),
        fingerprint: fingerprint.digest(),
        slice: "identity-user-registration".to_string(),
        source: "legacy".to_string(),
        adapter: layout_adapter.to_string(),
        operation: SourceOperation::Extract,
        inputs: Some(fingerprint.clone()),
    }
}

#[test]
fn write_then_lookup_is_a_hit() {
    let dir = tempdir().expect("tempdir");
    let layout = CacheLayout::new(dir.path(), "typescript");
    let fingerprint = fp("typescript@1");
    let digest = fingerprint.digest();
    let entry = index_entry("typescript", &fingerprint);

    // Cold-start: miss with no-prior-entry.
    let cold = lookup(layout, &fingerprint, None, &entry.slice, &entry.source, entry.operation)
        .expect("cold lookup");
    assert!(matches!(
        cold.outcome,
        LookupOutcome::Miss {
            reason: CacheMissReason::NoPriorEntry
        }
    ));

    write(layout, &fingerprint, b"---\nclaims: []\n", "evidence.yaml", &entry).expect("write");

    let warm = lookup(layout, &fingerprint, None, &entry.slice, &entry.source, entry.operation)
        .expect("warm lookup");
    match warm.outcome {
        LookupOutcome::Hit { cache_dir } => {
            assert!(cache_dir.is_dir(), "hit cache_dir must exist: {}", cache_dir.display());
            assert!(cache_dir.join("evidence.yaml").is_file(), "artifact persisted");
        }
        LookupOutcome::Miss { reason } => panic!("expected Hit, got Miss({reason})"),
    }
    let entries = read_index(layout).expect("read index");
    assert_eq!(entries.len(), 1, "one row per cache write");
    assert_eq!(entries[0].fingerprint, digest);
}

#[test]
fn adapter_opt_out_misses() {
    let dir = tempdir().expect("tempdir");
    let layout = CacheLayout::new(dir.path(), "doc");
    let fingerprint = fp("doc@1");
    let entry = index_entry("doc", &fingerprint);

    // Opt-out short-circuits before any filesystem read; the caller
    // also skips `write`, so the extraction tree never materialises
    // for an opt-out adapter.
    let outcome = lookup(
        layout,
        &fingerprint,
        Some(CacheMode::OptOut),
        &entry.slice,
        &entry.source,
        entry.operation,
    )
    .expect("opt-out lookup");
    assert!(matches!(
        outcome.outcome,
        LookupOutcome::Miss {
            reason: CacheMissReason::AdapterOptOut
        }
    ));
    assert!(!layout.adapter_dir().exists(), "opt-out lookup must not touch the cache tree");
}

#[test]
fn version_bump_reports_changed_reason() {
    let dir = tempdir().expect("tempdir");
    let layout = CacheLayout::new(dir.path(), "typescript");
    let v1 = fp("typescript@1");
    let v2 = fp("typescript@2");
    let entry_v1 = index_entry("typescript", &v1);

    write(layout, &v1, b"e1", "evidence.yaml", &entry_v1).expect("write v1");

    let outcome = lookup(layout, &v2, None, &entry_v1.slice, &entry_v1.source, entry_v1.operation)
        .expect("v2 lookup");
    match outcome.outcome {
        LookupOutcome::Miss { reason } => {
            assert_eq!(reason, CacheMissReason::AdapterVersionChanged);
        }
        LookupOutcome::Hit { cache_dir } => {
            panic!("expected miss, got Hit({})", cache_dir.display())
        }
    }
}

#[test]
fn inputless_prior_row_is_no_prior_entry() {
    let dir = tempdir().expect("tempdir");
    let layout = CacheLayout::new(dir.path(), "typescript");
    let prior = fp("typescript@1");
    let mut entry = index_entry("typescript", &prior);
    entry.inputs = None;
    write(layout, &prior, b"e1", "evidence.yaml", &entry).expect("write");

    let next = fp("typescript@2");
    let outcome = lookup(layout, &next, None, &entry.slice, &entry.source, entry.operation)
        .expect("lookup on inputless prior");
    assert!(matches!(
        outcome.outcome,
        LookupOutcome::Miss {
            reason: CacheMissReason::NoPriorEntry
        }
    ));
}

#[test]
fn index_read_skips_blanks_rejects_garbage() {
    let dir = tempdir().expect("tempdir");
    let layout = CacheLayout::new(dir.path(), "typescript");
    std::fs::create_dir_all(layout.adapter_dir()).expect("mkdir");
    std::fs::write(
        layout.index_path(),
        "{\"timestamp\":\"2026-05-22T13:15:00Z\",\"fingerprint\":\"sha256:a\",\"slice\":\"s\",\"source\":\"k\",\"adapter\":\"a\",\"operation\":\"extract\"}\n\n",
    )
    .expect("write index");
    let rows = read_index(layout).expect("read index");
    assert_eq!(rows.len(), 1);

    std::fs::write(layout.index_path(), "garbage\n").expect("clobber");
    let err = read_index(layout).expect_err("malformed index");
    match err {
        Error::Diag { code, .. } => assert_eq!(code, "cache-index-malformed"),
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn digest_dir_name_strips_sha256_prefix() {
    assert_eq!(digest_dir_name("sha256:abc"), "abc");
    assert_eq!(digest_dir_name("abc"), "abc");
}
