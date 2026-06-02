use tempfile::tempdir;

use super::*;
use crate::journal::test_timestamp;
use crate::slice::LifecycleStatus;

fn sample() -> SliceMetadata {
    SliceMetadata {
        target: "omnia".to_string(),
        status: LifecycleStatus::Refined,
        created_at: Some(test_timestamp("2024-08-01T10:00:00Z")),
        defined_at: Some(test_timestamp("2024-08-01T12:00:00Z")),
        completed_at: Some(test_timestamp("2024-08-03T15:45:00Z")),
        merged_at: None,
        dropped_at: None,
        drop_reason: None,
        touched_specs: vec![
            TouchedSpec {
                name: "login".to_string(),
                kind: SpecKind::Modified,
            },
            TouchedSpec {
                name: "oauth".to_string(),
                kind: SpecKind::New,
            },
        ],
        outcome: None,
    }
}

#[test]
fn save_load_round_trips() {
    let dir = tempdir().expect("tempdir");
    let meta = sample();
    meta.save(dir.path()).expect("save ok");
    let loaded = SliceMetadata::load(dir.path()).expect("load ok");
    assert_eq!(loaded, meta);
}
