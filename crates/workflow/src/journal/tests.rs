use tempfile::tempdir;

use super::{wire_shapes, *};

fn read_lines(layout: Layout<'_>) -> Vec<String> {
    let raw = std::fs::read_to_string(path(layout)).expect("read journal");
    raw.lines().map(str::to_owned).collect()
}

#[test]
fn append_creates_specify_dir_when_missing() {
    let dir = tempdir().expect("tempdir");
    let layout = Layout::new(dir.path());
    assert!(!layout.specify_dir().exists(), "precondition: .specify must not exist yet");

    let event = Event::new(
        test_timestamp("2026-05-21T20:02:00Z"),
        EventKind::SliceTransitionRefined {
            slice_name: "checkout".into(),
        },
    );
    append_batch(layout, std::slice::from_ref(&event)).expect("append ok");

    assert!(layout.specify_dir().is_dir(), ".specify/ must exist after first append");
    assert!(path(layout).is_file(), "journal.jsonl must exist after first append");
}

#[test]
fn append_batch_writes_in_order() {
    // auto-approve Gate-1 contract: `specrun plan create --auto-approve
    // --authority-override` may emit both `plan.transition.approved`
    // and `plan.amend.authority-override` in a single fsynced append.
    // Exercise the batched helper to lock ordering.
    let dir = tempdir().expect("tempdir");
    let layout = Layout::new(dir.path());
    let events = vec![
        Event::new(
            test_timestamp("2026-05-22T13:30:00Z"),
            EventKind::PlanTransitionApproved {
                plan_name: "fresh".into(),
            },
        ),
        Event::new(
            test_timestamp("2026-05-22T13:30:00Z"),
            EventKind::PlanAmendAuthorityOverride {
                plan_name: "fresh".into(),
                slice_name: "checkout".into(),
                action: AuthorityOverrideAction::Set,
                claim_kind: Some("criterion".to_string()),
                source: Some("runtime".to_string()),
            },
        ),
    ];
    append_batch(layout, &events).expect("append_batch ok");

    let lines = read_lines(layout);
    assert_eq!(lines.len(), 2, "expected two journal lines, got {}", lines.len());
    assert!(
        lines[0].contains(r#""event":"plan.transition.approved""#),
        "first line must be plan.transition.approved, got:\n{}",
        lines[0]
    );
    assert!(
        lines[1].contains(r#""event":"plan.amend.authority-override""#),
        "second line must be plan.amend.authority-override, got:\n{}",
        lines[1]
    );
}

#[test]
fn append_batch_empty_slice_is_no_op() {
    // Callers (e.g. `plan create` without `--auto-approve` and
    // without `--authority-override`) build the batch
    // unconditionally; an empty input must not create the
    // journal file on disk.
    let dir = tempdir().expect("tempdir");
    let layout = Layout::new(dir.path());
    append_batch(layout, &[]).expect("empty batch ok");
    assert!(
        !path(layout).exists(),
        "empty append_batch must not create journal.jsonl, found: {}",
        path(layout).display()
    );
}

#[test]
fn event_wire_shapes_match_contract() {
    wire_shapes::check_contract_part1();
    wire_shapes::check_contract_part2();
    wire_shapes::check_contract_part3();
    wire_shapes::check_contract_part4();
}

#[test]
fn cache_miss_reason_round_trips() {
    for (variant, wire) in [
        (CacheMissReason::NoPriorEntry, "no-prior-entry"),
        (CacheMissReason::SourcePathChanged, "source-path-changed"),
        (CacheMissReason::AdapterVersionChanged, "adapter-version-changed"),
        (CacheMissReason::BriefShaChanged, "brief-sha-changed"),
        (CacheMissReason::ToolVersionChanged, "tool-version-changed"),
        (CacheMissReason::AdapterOptOut, "adapter-opt-out"),
    ] {
        assert_eq!(serde_json::to_string(&variant).expect("serialise"), format!("\"{wire}\""));
    }
}

#[test]
fn plan_reconcile_event_round_trips() {
    // `specrun plan propose --from` emits one `plan.reconcile.completed` event; lock its wire shape.
    let completed = Event::new(
        test_timestamp("2026-05-22T13:15:00Z"),
        EventKind::PlanReconcileCompleted {
            plan_name: "identity-revamp".into(),
            slice_count: 3,
            slice_names: vec![
                "identity-contracts".into(),
                "identity-service".into(),
                "password-reset".into(),
            ],
        },
    );
    let completed_json = serde_json::to_string(&completed).expect("serialise completed");
    for needle in [
        r#""event":"plan.reconcile.completed""#,
        r#""plan-name":"identity-revamp""#,
        r#""slice-count":3"#,
        r#""slice-names":["identity-contracts","identity-service","password-reset"]"#,
    ] {
        assert!(
            completed_json.contains(needle),
            "completed wire form must contain `{needle}`; got:\n{completed_json}"
        );
    }
    let completed_round: Event =
        serde_json::from_str(&completed_json).expect("deserialise completed");
    assert_eq!(completed_round, completed, "completed round-trip must preserve every field");
}

#[test]
fn slice_synthesize_events_round_trip() {
    let rows: &[(EventKind, &[&str])] = &[
        (
            EventKind::SliceSynthesizeStarted {
                slice_name: "identity-user-registration".into(),
            },
            &[
                r#""event":"slice.synthesize.started""#,
                r#""slice-name":"identity-user-registration""#,
            ],
        ),
        (
            EventKind::SliceSynthesizeAgent {
                slice_name: "identity-user-registration".into(),
            },
            &[
                r#""event":"slice.synthesize.agent""#,
                r#""slice-name":"identity-user-registration""#,
            ],
        ),
        (
            EventKind::SliceSynthesizeCompleted {
                slice_name: "identity-user-registration".into(),
                artifacts: vec![
                    "proposal.md".to_string(),
                    "specs/identity/spec.md".to_string(),
                    "design.md".to_string(),
                    "tasks.md".to_string(),
                    "model.yaml".to_string(),
                ],
            },
            &[
                r#""event":"slice.synthesize.completed""#,
                r#""slice-name":"identity-user-registration""#,
                r#""artifacts":["proposal.md","specs/identity/spec.md","design.md","tasks.md","model.yaml"]"#,
            ],
        ),
        (
            EventKind::SliceSynthesizeFailed {
                slice_name: "identity-user-registration".into(),
                reason: "spec-requirement-missing-sources".to_string(),
            },
            &[
                r#""event":"slice.synthesize.failed""#,
                r#""slice-name":"identity-user-registration""#,
                r#""reason":"spec-requirement-missing-sources""#,
            ],
        ),
    ];
    wire_shapes::assert_wire_rows(rows);
}

#[test]
fn slice_build_merge_events_round_trip() {
    let rows: &[(EventKind, &[&str])] = &[
        (
            EventKind::SliceBuildStarted {
                slice_name: "identity-user-registration".into(),
            },
            &[r#""event":"slice.build.started""#, r#""slice-name":"identity-user-registration""#],
        ),
        (
            EventKind::SliceBuildSucceeded {
                slice_name: "identity-user-registration".into(),
            },
            &[r#""event":"slice.build.succeeded""#, r#""slice-name":"identity-user-registration""#],
        ),
        (
            EventKind::SliceBuildFailed {
                slice_name: "identity-user-registration".into(),
                reason: "cargo-check-failed".to_string(),
            },
            &[
                r#""event":"slice.build.failed""#,
                r#""slice-name":"identity-user-registration""#,
                r#""reason":"cargo-check-failed""#,
            ],
        ),
        (
            EventKind::SliceMergeStarted {
                slice_name: "identity-user-registration".into(),
            },
            &[r#""event":"slice.merge.started""#, r#""slice-name":"identity-user-registration""#],
        ),
        (
            EventKind::SliceMergeSucceeded {
                slice_name: "identity-user-registration".into(),
            },
            &[r#""event":"slice.merge.succeeded""#, r#""slice-name":"identity-user-registration""#],
        ),
        (
            EventKind::SliceMergeFailed {
                slice_name: "identity-user-registration".into(),
                reason: "baseline-conflict".to_string(),
            },
            &[
                r#""event":"slice.merge.failed""#,
                r#""slice-name":"identity-user-registration""#,
                r#""reason":"baseline-conflict""#,
            ],
        ),
        (
            EventKind::TargetExecutionAgent {
                slice: "identity-user-registration".into(),
                target: "omnia".to_string(),
            },
            &[
                r#""event":"target.execution.agent""#,
                r#""slice":"identity-user-registration""#,
                r#""target":"omnia""#,
            ],
        ),
    ];
    wire_shapes::assert_wire_rows(rows);
}

#[test]
fn cli_plugins_migration_events_round_trip() {
    let rows: &[(EventKind, &[&str])] = &[
        (
            EventKind::CliUpgraded {
                from: "1.4.0".to_string(),
                to: "1.5.0".to_string(),
                channel: "brew".to_string(),
            },
            &[
                r#""event":"cli.upgraded""#,
                r#""from":"1.4.0""#,
                r#""to":"1.5.0""#,
                r#""channel":"brew""#,
            ],
        ),
        (
            EventKind::PluginsRefreshed {
                deleted_paths: vec![
                    ".cursor/plugins/cache/augentic/spec".to_string(),
                    ".cursor/plugins/cache/augentic/capture".to_string(),
                ],
                marketplace: ".cursor-plugin/marketplace.json".to_string(),
            },
            &[
                r#""event":"plugins.refreshed""#,
                r#""deleted-paths":[".cursor/plugins/cache/augentic/spec",".cursor/plugins/cache/augentic/capture"]"#,
                r#""marketplace":".cursor-plugin/marketplace.json""#,
            ],
        ),
        (
            EventKind::MigrationApplied {
                kind: "v1-to-v2".to_string(),
                files_rewritten: 7,
                files_moved: 3,
            },
            &[
                r#""event":"migration.applied""#,
                r#""kind":"v1-to-v2""#,
                r#""files-rewritten":7"#,
                r#""files-moved":3"#,
            ],
        ),
        (
            EventKind::MigrationSkipped {
                kind: "v1-to-v2".to_string(),
                reason: "staged-validation-failed".to_string(),
            },
            &[
                r#""event":"migration.skipped""#,
                r#""kind":"v1-to-v2""#,
                r#""reason":"staged-validation-failed""#,
            ],
        ),
    ];
    wire_shapes::assert_wire_rows(rows);
}

#[test]
fn slice_synthesize_completed_omits_empty_artifacts() {
    // `artifacts` carries `skip_serializing_if = "Vec::is_empty"`
    // so an empty list does not reach the wire at all.
    let event = Event::new(
        test_timestamp("2026-05-22T13:15:00Z"),
        EventKind::SliceSynthesizeCompleted {
            slice_name: "identity-user-registration".into(),
            artifacts: vec![],
        },
    );
    let json = serde_json::to_string(&event).expect("serialise");
    assert!(!json.contains("artifacts"), "empty artifacts must not reach the wire; got:\n{json}");
    let round: Event = serde_json::from_str(&json).expect("deserialise");
    assert_eq!(round, event, "round-trip must preserve the empty artifacts list");
}

#[test]
fn lint_completed_round_trips() {
    // The lint-completed payload uses snake_case wire fields
    // (`duration_ms`, `baseline_present`, `false_positive`,
    // `exit_code`) so the JSON matches the payload example
    // verbatim. The wire id itself stays dotted-kebab.
    let event = Event::new(
        test_timestamp("2026-05-22T13:15:00Z"),
        EventKind::LintCompleted(LintCompletedPayload {
            scope: LintScope {
                target: Some("omnia".to_string()),
                slice: None,
                artifact: None,
            },
            duration_ms: 824,
            counts: LintCounts {
                open: 12,
                ignored: 4,
                false_positive: 0,
            },
            baseline_present: false,
            exit_code: 2,
        }),
    );

    let json = serde_json::to_string(&event).expect("serialise lint-completed");
    let round_tripped: Event = serde_json::from_str(&json).expect("deserialise lint-completed");
    assert_eq!(round_tripped, event, "round-trip must preserve every field");

    for needle in [
        r#""event":"lint-completed""#,
        r#""scope":{"target":"omnia","slice":null,"artifact":null}"#,
        r#""duration_ms":824"#,
        r#""open":12"#,
        r#""ignored":4"#,
        r#""false_positive":0"#,
        r#""baseline_present":false"#,
        r#""exit_code":2"#,
    ] {
        assert!(
            json.contains(needle),
            "lint-completed wire form must contain `{needle}`; got:\n{json}"
        );
    }

    // Guard against an accidental rename_all = "kebab-case" on the payload structs.
    for forbidden in
        [r#""duration-ms""#, r#""baseline-present""#, r#""false-positive""#, r#""exit-code""#]
    {
        assert!(
            !json.contains(forbidden),
            "lint-completed wire form must NOT contain kebab-case `{forbidden}`; got:\n{json}"
        );
    }
}

#[test]
fn no_snake_case_leaks_to_wire() {
    let dir = tempdir().expect("tempdir");
    let layout = Layout::new(dir.path());
    wire_shapes::run_snake_case_probe(layout);
}
