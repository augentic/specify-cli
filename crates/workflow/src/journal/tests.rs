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
fn append_failure_records_dropped_sidecar() {
    // Journal events are observability, not the source of truth: a failed
    // primary append must not crash the verb, but it must not vanish either.
    // Force `append_batch` to fail by making `journal.jsonl` a directory so
    // the append `open()` errors, then assert `emit_best_effort` swallowed
    // the failure AND captured the event in the `.specify/journal.dropped`
    // sidecar.
    let dir = tempdir().expect("tempdir");
    let layout = Layout::new(dir.path());
    std::fs::create_dir_all(path(layout)).expect("create journal.jsonl as a directory");

    emit_best_effort(
        layout,
        EventKind::SliceMergeSucceeded {
            slice_name: "checkout".into(),
        },
        "slice.merge",
    );

    let sidecar = layout.specify_dir().join(DROPPED_FILE_NAME);
    let raw = std::fs::read_to_string(&sidecar).expect("dropped sidecar written on append failure");
    assert!(
        raw.contains(r#""event":"slice.merge.succeeded""#),
        "sidecar must capture the dropped event line; got:\n{raw}"
    );
    assert!(raw.ends_with('\n'), "sidecar line must be newline-terminated; got:\n{raw}");
}

#[test]
fn dropped_sidecar_appends_event_lines() {
    // The sidecar writer is append-only: repeated drops accumulate one
    // newline-terminated JSON line each, so the recovery trail preserves
    // every dropped event in order.
    let dir = tempdir().expect("tempdir");
    let layout = Layout::new(dir.path());
    let event = Event::new(
        test_timestamp("2026-05-21T20:02:00Z"),
        EventKind::SliceTransitionRefined {
            slice_name: "checkout".into(),
        },
    );
    append_dropped(layout, &event).expect("first sidecar append");
    append_dropped(layout, &event).expect("second sidecar append");

    let raw = std::fs::read_to_string(layout.specify_dir().join(DROPPED_FILE_NAME))
        .expect("read sidecar");
    assert_eq!(raw.lines().count(), 2, "two appends must yield two sidecar lines; got:\n{raw}");
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
fn synthesize_omits_empty_artifacts() {
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

/// Collect every line `for_each_line_rev` yields, restored to file order.
fn collect_chronological(path: &Path, chunk: usize) -> Vec<String> {
    let mut newest_first: Vec<String> = Vec::new();
    for_each_line_rev(path, chunk, |line| {
        newest_first.push(line.to_owned());
        true
    })
    .expect("for_each_line_rev");
    newest_first.reverse();
    newest_first
}

#[test]
fn for_each_line_rev_matches_str_lines() {
    // The tail reader must agree with `str::lines` for every line-boundary
    // shape — trailing newline or not, interior and trailing blanks, empty
    // file — across chunk sizes that force partial-line splits at the edge,
    // including multi-byte UTF-8 straddling a chunk boundary.
    let cases = [
        "",
        "a",
        "a\n",
        "a\nb\nc",
        "a\nb\nc\n",
        "\n",
        "\n\n",
        "a\n\nb\n",
        "a\n\n",
        "\na\n",
        "αβ\nγδ\nεζ",
        "αβ\nγδ\nεζ\n",
    ];
    let dir = tempdir().expect("tempdir");
    for (idx, content) in cases.iter().enumerate() {
        let path = dir.path().join(format!("case-{idx}.jsonl"));
        std::fs::write(&path, content).expect("write case");
        let expected: Vec<String> = content.lines().map(str::to_owned).collect();
        for chunk in [1_usize, 2, 3, 5, 8192] {
            assert_eq!(
                collect_chronological(&path, chunk),
                expected,
                "content {content:?} at chunk {chunk} must match str::lines"
            );
        }
    }
}

#[test]
fn for_each_line_rev_missing_file() {
    let dir = tempdir().expect("tempdir");
    let path = dir.path().join("absent.jsonl");
    let mut visited = 0_usize;
    for_each_line_rev(&path, 8192, |_| {
        visited += 1;
        true
    })
    .expect("missing file is not an error");
    assert_eq!(visited, 0, "a missing file must yield no lines");
}

#[test]
fn for_each_line_rev_early_stop() {
    // Returning `false` halts the backward scan; only the tail is touched.
    let dir = tempdir().expect("tempdir");
    let path = dir.path().join("early.jsonl");
    std::fs::write(&path, "first\nsecond\nthird\n").expect("write");
    let mut seen: Vec<String> = Vec::new();
    for_each_line_rev(&path, 8192, |line| {
        seen.push(line.to_owned());
        false
    })
    .expect("for_each_line_rev");
    assert_eq!(seen, vec!["third".to_string()], "early stop must yield only the newest line");
}

fn write_archive_summaries(layout: Layout<'_>, summaries: &[&str]) {
    for summary in summaries {
        let event = Event::new(
            test_timestamp("2026-05-22T13:15:00Z"),
            EventKind::SliceArchiveCreated {
                slice_name: "slice".into(),
                touched_specs: vec![],
                outcome_summary: (*summary).to_string(),
                merge_sha: None,
                decisions: vec![],
            },
        );
        append_batch(layout, std::slice::from_ref(&event)).expect("append archive event");
    }
}

#[test]
fn read_recent_last_n_matching() {
    let dir = tempdir().expect("tempdir");
    let layout = Layout::new(dir.path());
    // Interleave non-matching events so the tail must filter, not just slice
    // the last N raw lines.
    for idx in 0..5 {
        write_archive_summaries(layout, &[&format!("merge-{idx}")]);
        let noise = Event::new(
            test_timestamp("2026-05-22T13:15:00Z"),
            EventKind::SliceBuildStarted {
                slice_name: "slice".into(),
            },
        );
        append_batch(layout, std::slice::from_ref(&noise)).expect("append noise");
    }

    let recent: Vec<String> = read_recent(layout, 3, |event| match event.kind {
        EventKind::SliceArchiveCreated { outcome_summary, .. } => Some(outcome_summary),
        _ => None,
    })
    .expect("read_recent");
    assert_eq!(
        recent,
        vec!["merge-2".to_string(), "merge-3".to_string(), "merge-4".to_string()],
        "must keep the last N matching summaries in append order"
    );
}

#[test]
fn read_recent_short_missing_and_zero_limit() {
    let dir = tempdir().expect("tempdir");
    let layout = Layout::new(dir.path());

    // Missing journal -> empty.
    let absent: Vec<String> = read_recent(layout, 10, |event| match event.kind {
        EventKind::SliceArchiveCreated { outcome_summary, .. } => Some(outcome_summary),
        _ => None,
    })
    .expect("read_recent missing");
    assert!(absent.is_empty(), "missing journal must yield no summaries");

    write_archive_summaries(layout, &["only"]);

    // Fewer matches than the limit -> all of them, in order.
    let short: Vec<String> = read_recent(layout, 10, |event| match event.kind {
        EventKind::SliceArchiveCreated { outcome_summary, .. } => Some(outcome_summary),
        _ => None,
    })
    .expect("read_recent short");
    assert_eq!(short, vec!["only".to_string()], "fewer-than-limit must return all matches");

    // Zero limit -> empty, no read needed.
    let none: Vec<String> = read_recent(layout, 0, |event| match event.kind {
        EventKind::SliceArchiveCreated { outcome_summary, .. } => Some(outcome_summary),
        _ => None,
    })
    .expect("read_recent zero");
    assert!(none.is_empty(), "a zero limit must yield no summaries");
}
