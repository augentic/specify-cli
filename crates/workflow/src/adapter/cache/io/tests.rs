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

fn index_entry(layout_adapter: &str, digest: &str) -> CacheIndexEntry {
    CacheIndexEntry {
        timestamp: test_timestamp("2026-05-22T13:15:00Z"),
        fingerprint: digest.to_string(),
        slice: "identity-user-registration".to_string(),
        source: "legacy".to_string(),
        adapter: layout_adapter.to_string(),
        operation: SourceOperation::Extract,
    }
}

#[test]
fn write_then_lookup_is_a_hit() {
    let dir = tempdir().expect("tempdir");
    let layout = CacheLayout::new(dir.path(), "code-typescript");
    let fingerprint = fp("code-typescript@1");
    let digest = fingerprint.digest();
    let entry = index_entry("code-typescript", &digest);

    // Cold-start: miss with no-prior-entry.
    let cold = lookup(layout, &fingerprint, None, &entry.slice, &entry.source, entry.operation)
        .expect("cold lookup");
    assert!(matches!(
        cold.outcome,
        LookupOutcome::Miss {
            reason: CacheMissReason::NoPriorEntry
        }
    ));

    write(layout, &fingerprint, b"---\nclaims: []\n", "evidence.yaml", None, &entry)
        .expect("write");

    let warm = lookup(layout, &fingerprint, None, &entry.slice, &entry.source, entry.operation)
        .expect("warm lookup");
    match warm.outcome {
        LookupOutcome::Hit { cache_dir } => {
            assert!(cache_dir.is_dir(), "hit cache_dir must exist: {}", cache_dir.display());
            assert!(cache_dir.join("evidence.yaml").is_file(), "artifact persisted");
            assert!(cache_dir.join("fingerprint.json").is_file(), "record persisted");
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
    let digest = fingerprint.digest();
    let entry = index_entry("doc", &digest);

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

    write(layout, &fingerprint, b"unused", "evidence.yaml", Some(CacheMode::OptOut), &entry)
        .expect("opt-out write still appends index");
    assert!(
        !layout.fingerprint_dir(&digest).exists(),
        "opt-out must not create the cache directory"
    );
    let entries = read_index(layout).expect("read index");
    assert_eq!(entries.len(), 1, "index still records the audit row under opt-out");
}

#[test]
fn adapter_version_bump_reports_changed_reason() {
    let dir = tempdir().expect("tempdir");
    let layout = CacheLayout::new(dir.path(), "code-typescript");
    let v1 = fp("code-typescript@1");
    let v2 = fp("code-typescript@2");
    let entry_v1 = index_entry("code-typescript", &v1.digest());

    write(layout, &v1, b"e1", "evidence.yaml", None, &entry_v1).expect("write v1");

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
fn corrupt_prior_record_ignored() {
    let dir = tempdir().expect("tempdir");
    let layout = CacheLayout::new(dir.path(), "code-typescript");
    let prior = fp("code-typescript@1");
    let entry = index_entry("code-typescript", &prior.digest());
    write(layout, &prior, b"e1", "evidence.yaml", None, &entry).expect("write");

    // Corrupt the prior fingerprint.json.
    let record_path = layout.fingerprint_record_path(&prior.digest());
    std::fs::write(&record_path, "{not json").expect("clobber record");

    let next = fp("code-typescript@2");
    let outcome = lookup(layout, &next, None, &entry.slice, &entry.source, entry.operation)
        .expect("lookup on corrupt prior");
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
    let layout = CacheLayout::new(dir.path(), "code-typescript");
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
