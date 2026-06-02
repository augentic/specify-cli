use std::fs;

use specify_digest::sha256_hex;
use tempfile::tempdir;

use super::*;

#[test]
fn resolve_empty_when_to_equals_from() {
    assert!(MigrationKind::resolve(2, 2).is_empty());
    assert!(MigrationKind::resolve(0, 0).is_empty());
}

#[test]
fn resolve_empty_when_to_below_from() {
    assert!(MigrationKind::resolve(2, 1).is_empty());
}

#[test]
fn resolve_single_hop() {
    assert_eq!(MigrationKind::resolve(1, 2), vec![MigrationKind::V1ToV2]);
}

#[test]
fn resolve_empty_when_hop_missing() {
    // Major 0 → 1 and 2 → 3 have no registered hop yet; the walk cannot
    // reach `to`, so the chain collapses to empty. Adding `V2ToV3` would
    // make `resolve(1, 3)` compose to `[V1ToV2, V2ToV3]`.
    assert!(MigrationKind::resolve(0, 1).is_empty());
    assert!(MigrationKind::resolve(1, 3).is_empty());
    assert!(MigrationKind::resolve(2, 3).is_empty());
}

#[test]
fn hops_form_contiguous_sorted_chain() {
    // The composition invariant: each hop advances by a positive span
    // and dovetails into the next (`hop.to == next.from`). Guards future
    // `HOPS` additions even though only one hop exists today.
    for window in HOPS.windows(2) {
        assert_eq!(window[0].to, window[1].from, "hops must dovetail for resolve() to compose");
    }
    for hop in HOPS {
        assert!(hop.to > hop.from, "each hop must advance the major version");
    }
}

#[test]
fn id_matches_serde_wire_form() {
    let wire = serde_json::to_value(MigrationKind::V1ToV2).expect("serialise kind");
    assert_eq!(wire, serde_json::json!("v1-to-v2"));
    assert_eq!(MigrationKind::V1ToV2.id(), "v1-to-v2");
}

#[test]
fn major_parses_or_none() {
    assert_eq!(major("1.2.3"), Some(1));
    assert_eq!(major("2.0.0"), Some(2));
    assert_eq!(major("not-a-semver"), None);
}

#[test]
fn apply_staged_empty_plan_is_skip() {
    let tmp = tempdir().unwrap();
    let plan = MigrationPlan::new(MigrationKind::V1ToV2, Vec::new());
    let report = apply_staged(tmp.path(), &plan).expect("empty plan applies");

    assert_eq!(report.status, MigrationStatus::Skipped);
    assert!(report.files.is_empty());
    assert_eq!(report.files_rewritten, 0);
    assert_eq!(report.files_moved, 0);
    assert!(!tmp.path().join(".specify").exists(), "empty plan must not touch the tree");
}

#[test]
fn apply_staged_moves_and_rewrites() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    fs::write(root.join("old.txt"), b"moved-bytes").unwrap();

    let plan = MigrationPlan::new(
        MigrationKind::V1ToV2,
        vec![
            MigrationAction::Rewrite {
                path: PathBuf::from("data.yaml"),
                contents: "new: value\n".to_string(),
            },
            MigrationAction::Move {
                from: PathBuf::from("old.txt"),
                to: PathBuf::from("nested/new.txt"),
            },
        ],
    );

    let report = apply_staged(root, &plan).expect("plan applies");

    assert_eq!(report.status, MigrationStatus::Applied);
    assert_eq!(report.files_rewritten, 1);
    assert_eq!(report.files_moved, 1);

    assert_eq!(fs::read_to_string(root.join("data.yaml")).unwrap(), "new: value\n");
    assert_eq!(fs::read_to_string(root.join("nested/new.txt")).unwrap(), "moved-bytes");
    assert!(!root.join("old.txt").exists(), "move source must be dropped");
    assert!(
        !root.join(".specify/.migrate/v1-to-v2").exists(),
        "per-migrator staging tree must be cleaned up after commit"
    );

    let rewrite = report.files.iter().find(|f| f.path == Path::new("data.yaml")).unwrap();
    assert_eq!(rewrite.change, FileChange::Rewritten);
    assert_eq!(rewrite.sha256, sha256_hex(b"new: value\n"));
    assert!(rewrite.from.is_none());

    let moved = report.files.iter().find(|f| f.path == Path::new("nested/new.txt")).unwrap();
    assert_eq!(moved.change, FileChange::Moved);
    assert_eq!(moved.sha256, sha256_hex(b"moved-bytes"));
    assert_eq!(moved.from.as_deref(), Some(Path::new("old.txt")));
}

#[test]
fn apply_staged_removes_and_reports() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    fs::write(root.join("doomed.txt"), b"delete-me").unwrap();
    fs::write(root.join("kept.txt"), b"keep").unwrap();

    let plan = MigrationPlan::new(
        MigrationKind::V1ToV2,
        vec![
            MigrationAction::Rewrite {
                path: PathBuf::from("kept.txt"),
                contents: "rewritten".to_string(),
            },
            MigrationAction::Remove {
                path: PathBuf::from("doomed.txt"),
            },
        ],
    );

    let report = apply_staged(root, &plan).expect("plan applies");
    assert_eq!(report.status, MigrationStatus::Applied);
    assert_eq!(report.files_rewritten, 1);
    assert_eq!(report.files_moved, 0);

    assert!(!root.join("doomed.txt").exists(), "removal target must be gone");
    assert_eq!(fs::read_to_string(root.join("kept.txt")).unwrap(), "rewritten");

    let removed = report.files.iter().find(|f| f.path == Path::new("doomed.txt")).unwrap();
    assert_eq!(removed.change, FileChange::Removed);
    assert_eq!(removed.sha256, sha256_hex(b"delete-me"));
    assert!(removed.from.is_none());
}

#[test]
fn apply_staged_missing_removal_target_leaves_tree_untouched() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    fs::write(root.join("data.yaml"), b"original").unwrap();

    // A valid rewrite followed by a removal whose target is missing.
    // The missing target is detected while resolving actions, before any
    // staging or commit, so the rewrite never lands.
    let plan = MigrationPlan::new(
        MigrationKind::V1ToV2,
        vec![
            MigrationAction::Rewrite {
                path: PathBuf::from("data.yaml"),
                contents: "rewritten".to_string(),
            },
            MigrationAction::Remove {
                path: PathBuf::from("missing.txt"),
            },
        ],
    );

    let err = apply_staged(root, &plan).expect_err("missing removal target aborts apply");
    assert!(matches!(
        err,
        Error::Filesystem {
            op: "migrate-read-source",
            ..
        }
    ));

    assert_eq!(fs::read_to_string(root.join("data.yaml")).unwrap(), "original");
    assert!(!root.join(".specify/.migrate").exists(), "no staging on precondition failure");
}

#[test]
fn apply_staged_precondition_failure_leaves_tree_untouched() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    fs::write(root.join("data.yaml"), b"original").unwrap();

    // A valid rewrite followed by a move whose source is missing. The
    // missing source is detected while resolving actions, before any
    // staging or commit, so the rewrite never lands.
    let plan = MigrationPlan::new(
        MigrationKind::V1ToV2,
        vec![
            MigrationAction::Rewrite {
                path: PathBuf::from("data.yaml"),
                contents: "rewritten".to_string(),
            },
            MigrationAction::Move {
                from: PathBuf::from("missing.txt"),
                to: PathBuf::from("dest.txt"),
            },
        ],
    );

    let err = apply_staged(root, &plan).expect_err("missing move source aborts apply");
    assert!(matches!(
        err,
        Error::Filesystem {
            op: "migrate-read-source",
            ..
        }
    ));

    assert_eq!(fs::read_to_string(root.join("data.yaml")).unwrap(), "original");
    assert!(!root.join("dest.txt").exists());
    assert!(!root.join(".specify/.migrate").exists(), "no staging on precondition failure");
}

#[test]
fn report_round_trips_through_kebab_wire() {
    let report = MigrationReport {
        kind: MigrationKind::V1ToV2,
        status: MigrationStatus::Applied,
        files: vec![FileOutcome {
            path: PathBuf::from("nested/new.txt"),
            change: FileChange::Moved,
            sha256: sha256_hex(b"moved-bytes"),
            from: Some(PathBuf::from("old.txt")),
        }],
        files_rewritten: 0,
        files_moved: 1,
    };

    let json = serde_json::to_value(&report).expect("serialise report");
    assert_eq!(json["kind"], "v1-to-v2");
    assert_eq!(json["files-rewritten"], 0);
    assert_eq!(json["files-moved"], 1);
    assert_eq!(json["files"][0]["change"], "moved");

    let back: MigrationReport = serde_json::from_value(json).expect("deserialise report");
    assert_eq!(back, report);
}
