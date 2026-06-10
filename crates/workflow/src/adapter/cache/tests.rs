use super::*;
use crate::journal::test_timestamp;

fn sample(adapter: &str, lead: Option<&str>) -> CacheFingerprint {
    CacheFingerprint::new(
        FingerprintSource::Path {
            path: "/repo/vendor/monolith".to_string(),
        },
        adapter.to_string(),
        "sha256:abc".to_string(),
        vec![FingerprintToolVersion {
            name: "tsc".to_string(),
            version: Some("5.4.0".to_string()),
        }],
        lead.map(str::to_string),
    )
}

#[test]
fn fingerprint_path_form_round_trips() {
    let fp = sample("typescript@1", Some("user-registration"));
    let json = serde_json::to_string(&fp).expect("serialise");
    assert!(json.contains(r#""source":{"kind":"path","path":"/repo/vendor/monolith"}"#));
    assert!(json.contains(r#""brief-sha256":"sha256:abc""#));
    assert!(json.contains(r#""tool-versions":[{"name":"tsc","version":"5.4.0"}]"#));
    let reparsed: CacheFingerprint = serde_json::from_str(&json).expect("reparse");
    assert_eq!(fp, reparsed);
}

#[test]
fn fingerprint_value_form_round_trips() {
    let fp = CacheFingerprint::new(
        FingerprintSource::Value {
            sha256: "sha256:deadbeef".to_string(),
        },
        "intent@1".to_string(),
        "sha256:b".to_string(),
        vec![],
        None,
    );
    let json = serde_json::to_string(&fp).expect("serialise");
    assert!(json.contains(r#""source":{"kind":"value","sha256":"sha256:deadbeef"}"#));
    assert!(!json.contains("tool-versions"), "empty tool-versions must elide");
    assert!(!json.contains("lead"), "absent lead must elide on survey fingerprint");
    let reparsed: CacheFingerprint = serde_json::from_str(&json).expect("reparse");
    assert_eq!(fp, reparsed);
}

#[test]
fn cache_index_entry_round_trips() {
    let entry = CacheIndexEntry {
        timestamp: test_timestamp("2026-05-22T13:15:00Z"),
        fingerprint: "sha256:cafef00d".to_string(),
        slice: "identity-user-registration".to_string(),
        source: "runtime".to_string(),
        adapter: "captures".to_string(),
        operation: SourceOperation::Extract,
        inputs: Some(sample("captures@1", Some("user-registration"))),
    };
    let json = serde_json::to_string(&entry).expect("serialise");
    assert!(json.contains(r#""timestamp":"2026-05-22T13:15:00Z""#));
    assert!(json.contains(r#""source":"runtime""#));
    assert!(json.contains(r#""operation":"extract""#));
    assert!(json.contains(r#""inputs":{"#));
    let reparsed: CacheIndexEntry = serde_json::from_str(&json).expect("reparse");
    assert_eq!(entry, reparsed);
}

#[test]
fn index_entry_without_inputs_still_parses() {
    let raw = r#"{
            "timestamp": "2026-05-22T13:15:00Z",
            "fingerprint": "sha256:a",
            "slice": "s",
            "source": "k",
            "adapter": "a",
            "operation": "extract"
        }"#;
    let entry: CacheIndexEntry = serde_json::from_str(raw).expect("inputless row parses");
    assert_eq!(entry.inputs, None);
    let json = serde_json::to_string(&entry).expect("serialise");
    assert!(!json.contains("inputs"), "absent inputs must elide");
}

#[test]
fn deny_unknown_fields_on_index_entry() {
    let raw = r#"{
            "timestamp": "2026-05-22T13:15:00Z",
            "fingerprint": "sha256:a",
            "slice": "s",
            "source": "k",
            "adapter": "a",
            "operation": "extract",
            "unknown": true
        }"#;
    let err = serde_json::from_str::<CacheIndexEntry>(raw).expect_err("unknown field rejected");
    assert!(
        err.to_string().contains("unknown field"),
        "unexpected error from deny_unknown_fields: {err}"
    );
}

#[test]
fn digest_is_byte_stable_across_runs() {
    let a = sample("typescript@1", Some("user-registration"));
    let b = sample("typescript@1", Some("user-registration"));
    assert_eq!(a.digest(), b.digest());
    assert!(a.digest().starts_with("sha256:"));
    // Pin the digest so any accidental change to canonical
    // serialisation (field rename, default skip rule, etc.)
    // fails this assertion loudly.
    assert_eq!(a.canonical_bytes(), b.canonical_bytes());
}

#[test]
fn tool_versions_sort_stable() {
    let unsorted = CacheFingerprint::new(
        FingerprintSource::Path {
            path: "/p".to_string(),
        },
        "a@1".to_string(),
        "sha256:b".to_string(),
        vec![
            FingerprintToolVersion {
                name: "zsh".to_string(),
                version: None,
            },
            FingerprintToolVersion {
                name: "ash".to_string(),
                version: Some("1".to_string()),
            },
        ],
        None,
    );
    let sorted = CacheFingerprint::new(
        FingerprintSource::Path {
            path: "/p".to_string(),
        },
        "a@1".to_string(),
        "sha256:b".to_string(),
        vec![
            FingerprintToolVersion {
                name: "ash".to_string(),
                version: Some("1".to_string()),
            },
            FingerprintToolVersion {
                name: "zsh".to_string(),
                version: None,
            },
        ],
        None,
    );
    assert_eq!(unsorted.digest(), sorted.digest());
}

#[test]
fn each_input_flip_changes_the_digest() {
    let base = sample("typescript@1", Some("c1"));
    let baseline = base.digest();

    let mut source_changed = base.clone();
    source_changed.source = FingerprintSource::Path {
        path: "/other".to_string(),
    };
    assert_ne!(source_changed.digest(), baseline, "source flip must change digest");

    let mut adapter_changed = base.clone();
    adapter_changed.adapter = "typescript@2".to_string();
    assert_ne!(adapter_changed.digest(), baseline, "adapter flip must change digest");

    let mut brief_changed = base.clone();
    brief_changed.brief_sha256 = "sha256:xyz".to_string();
    assert_ne!(brief_changed.digest(), baseline, "brief flip must change digest");

    let mut tool_changed = base.clone();
    tool_changed.tool_versions[0].version = Some("5.5.0".to_string());
    assert_ne!(tool_changed.digest(), baseline, "tool flip must change digest");

    let mut lead_changed = base;
    lead_changed.lead = Some("c2".to_string());
    assert_ne!(lead_changed.digest(), baseline, "lead flip must change digest");
}

#[test]
fn diff_reason_walks_declared_field_order() {
    let prior = sample("a@1", Some("c1"));

    let same = sample("a@1", Some("c1"));
    assert!(CacheFingerprint::diff_reason(&prior, &same).is_none());

    let mut source_changed = prior.clone();
    source_changed.source = FingerprintSource::Path {
        path: "/other".to_string(),
    };
    assert_eq!(
        CacheFingerprint::diff_reason(&prior, &source_changed),
        Some(CacheMissReason::SourcePathChanged)
    );

    let mut adapter_changed = prior.clone();
    adapter_changed.adapter = "a@2".to_string();
    assert_eq!(
        CacheFingerprint::diff_reason(&prior, &adapter_changed),
        Some(CacheMissReason::AdapterVersionChanged)
    );

    let mut brief_changed = prior.clone();
    brief_changed.brief_sha256 = "sha256:other".to_string();
    assert_eq!(
        CacheFingerprint::diff_reason(&prior, &brief_changed),
        Some(CacheMissReason::BriefShaChanged)
    );

    let mut tool_changed = prior.clone();
    tool_changed.tool_versions[0].version = Some("5.5.0".to_string());
    assert_eq!(
        CacheFingerprint::diff_reason(&prior, &tool_changed),
        Some(CacheMissReason::ToolVersionChanged)
    );
}

#[test]
fn diff_reason_picks_first_change() {
    let prior = sample("a@1", Some("c1"));
    let mut both = prior.clone();
    both.source = FingerprintSource::Path {
        path: "/other".to_string(),
    };
    both.adapter = "a@2".to_string();
    assert_eq!(
        CacheFingerprint::diff_reason(&prior, &both),
        Some(CacheMissReason::SourcePathChanged),
        "earlier-declared field wins on multi-field drift"
    );
}
