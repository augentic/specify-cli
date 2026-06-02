use tempfile::tempdir;

use super::*;

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
            slice_name: "checkout".to_string(),
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
                plan_name: "fresh".to_string(),
            },
        ),
        Event::new(
            test_timestamp("2026-05-22T13:30:00Z"),
            EventKind::PlanAmendAuthorityOverride {
                plan_name: "fresh".to_string(),
                slice_name: "checkout".to_string(),
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
#[expect(
    clippy::too_many_lines,
    reason = "Single table pins every payload-bearing variant's wire shape; splitting hides the contract."
)]
fn event_wire_shapes_match_contract() {
    let dir = tempdir().expect("tempdir");
    let layout = Layout::new(dir.path());
    let rows: &[(EventKind, &[&str])] = &[
        (
            EventKind::SliceExtractCacheHit {
                slice_name: "identity-user-registration".to_string(),
                source: "runtime".to_string(),
                adapter: "captures".to_string(),
                fingerprint: "sha256:cafef00d".to_string(),
            },
            &[
                r#"{"timestamp":"2026-05-22T13:15:00Z","event":"slice.extract.cache-hit","payload":{"slice-name":"identity-user-registration","source":"runtime","adapter":"captures","fingerprint":"sha256:cafef00d"}}"#,
            ],
        ),
        (
            EventKind::SliceExtractCacheMiss {
                slice_name: "identity-user-registration".to_string(),
                source: "runtime".to_string(),
                adapter: "captures".to_string(),
                fingerprint: "sha256:beef".to_string(),
                reason: CacheMissReason::AdapterVersionChanged,
            },
            &[
                r#""event":"slice.extract.cache-miss""#,
                r#""reason":"adapter-version-changed""#,
                r#""source":"runtime""#,
            ],
        ),
        (
            EventKind::SliceReplayCompleted {
                slice_name: "identity-user-registration".to_string(),
                runner: "omnia-target@1.4 (cargo nextest)".to_string(),
                passed: 47,
                failed: 0,
                skipped: 0,
            },
            &[
                r#""event":"slice.replay.completed""#,
                r#""passed":47"#,
                r#""failed":0"#,
                r#""skipped":0"#,
                r#""runner":"omnia-target@1.4 (cargo nextest)""#,
            ],
        ),
        (
            EventKind::PlanAmendAuthorityOverride {
                plan_name: "identity-revamp".to_string(),
                slice_name: "identity-user-registration".to_string(),
                action: AuthorityOverrideAction::Set,
                claim_kind: Some("criterion".to_string()),
                source: Some("runtime".to_string()),
            },
            &[
                r#""event":"plan.amend.authority-override""#,
                r#""action":"set""#,
                r#""claim-kind":"criterion""#,
                r#""source":"runtime""#,
            ],
        ),
        (
            EventKind::SourceSurveyCacheHit {
                source: "runtime".to_string(),
                adapter: "captures".to_string(),
                fingerprint: "sha256:cafef00d".to_string(),
            },
            &[
                r#"{"timestamp":"2026-05-22T13:15:00Z","event":"source.survey.cache-hit","payload":{"source":"runtime","adapter":"captures","fingerprint":"sha256:cafef00d"}}"#,
            ],
        ),
        (
            EventKind::SourceSurveyCacheMiss {
                source: "runtime".to_string(),
                adapter: "captures".to_string(),
                fingerprint: "sha256:beef".to_string(),
                reason: CacheMissReason::AdapterOptOut,
            },
            &[
                r#""event":"source.survey.cache-miss""#,
                r#""reason":"adapter-opt-out""#,
                r#""source":"runtime""#,
                r#""fingerprint":"sha256:beef""#,
            ],
        ),
        (
            EventKind::SourceExecutionAgent {
                source: "runtime".to_string(),
                adapter: "captures".to_string(),
                operation: SourceOperation::Survey,
            },
            &[
                r#""event":"source.execution.agent""#,
                r#""operation":"survey""#,
                r#""source":"runtime""#,
                r#""adapter":"captures""#,
            ],
        ),
        (
            EventKind::PlanReconcileCompleted {
                plan_name: "identity-revamp".to_string(),
                slice_count: 3,
                slice_names: vec![
                    "identity-contracts".to_string(),
                    "identity-service".to_string(),
                    "password-reset".to_string(),
                ],
            },
            &[
                r#""event":"plan.reconcile.completed""#,
                r#""plan-name":"identity-revamp""#,
                r#""slice-count":3"#,
                r#""slice-names":["identity-contracts","identity-service","password-reset"]"#,
            ],
        ),
        (
            EventKind::SliceArchiveCreated {
                slice_name: "identity-service".to_string(),
                touched_specs: vec!["identity".to_string()],
                outcome_summary: "identity: 2 modified".to_string(),
                merge_sha: Some("a1b2c3d".to_string()),
            },
            &[
                r#""event":"slice.archive.created""#,
                r#""slice-name":"identity-service""#,
                r#""touched-specs":["identity"]"#,
                r#""outcome-summary":"identity: 2 modified""#,
                r#""merge-sha":"a1b2c3d""#,
            ],
        ),
    ];

    for (kind, required) in rows {
        let event = Event::new(test_timestamp("2026-05-22T13:15:00Z"), kind.clone());
        append_batch(layout, std::slice::from_ref(&event)).expect("append ok");
        let line = read_lines(layout).pop().expect("at least one line");
        if required.len() == 1 && required[0].starts_with('{') {
            assert_eq!(line, required[0]);
        } else {
            for needle in *required {
                assert!(line.contains(needle), "line must contain `{needle}`, got:\n{line}");
            }
        }
    }
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
    // `specrun plan propose --from` emits one `plan.reconcile.completed`
    // event (RFC-29 review F8 folded the former agent/completed pair
    // into this single indivisible event); lock its wire shape.
    let completed = Event::new(
        test_timestamp("2026-05-22T13:15:00Z"),
        EventKind::PlanReconcileCompleted {
            plan_name: "identity-revamp".to_string(),
            slice_count: 3,
            slice_names: vec![
                "identity-contracts".to_string(),
                "identity-service".to_string(),
                "password-reset".to_string(),
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
    // RFC-29c §"Wire contracts": the four M2b lifecycle events
    // serialise to their dotted-kebab ids with kebab-case payload
    // fields, and round-trip back preserving every field. Distinct
    // from the per-requirement `slice.synthesis.*` tag events.
    let rows: &[(EventKind, &[&str])] = &[
        (
            EventKind::SliceSynthesizeStarted {
                slice_name: "identity-user-registration".to_string(),
            },
            &[
                r#""event":"slice.synthesize.started""#,
                r#""slice-name":"identity-user-registration""#,
            ],
        ),
        (
            EventKind::SliceSynthesizeAgent {
                slice_name: "identity-user-registration".to_string(),
            },
            &[
                r#""event":"slice.synthesize.agent""#,
                r#""slice-name":"identity-user-registration""#,
            ],
        ),
        (
            EventKind::SliceSynthesizeCompleted {
                slice_name: "identity-user-registration".to_string(),
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
                slice_name: "identity-user-registration".to_string(),
                reason: "spec-requirement-missing-sources".to_string(),
            },
            &[
                r#""event":"slice.synthesize.failed""#,
                r#""slice-name":"identity-user-registration""#,
                r#""reason":"spec-requirement-missing-sources""#,
            ],
        ),
    ];

    for (kind, required) in rows {
        let event = Event::new(test_timestamp("2026-05-22T13:15:00Z"), kind.clone());
        let json = serde_json::to_string(&event).expect("serialise synthesize event");
        for needle in *required {
            assert!(json.contains(needle), "wire form must contain `{needle}`; got:\n{json}");
        }
        let round: Event = serde_json::from_str(&json).expect("deserialise synthesize event");
        assert_eq!(round, event, "synthesize round-trip must preserve every field");
    }
}

#[test]
fn slice_build_merge_events_round_trip() {
    // RFC-29d §"Journal events": the M3 build/merge lifecycle
    // events and `target.execution.agent` serialise to their
    // dotted-kebab ids with kebab-case payload fields, and
    // round-trip back preserving every field. The `*.failed`
    // variants carry a `reason`; `target.execution.agent` carries
    // the minimal `{ slice, target }` derived at build time.
    let rows: &[(EventKind, &[&str])] = &[
        (
            EventKind::SliceBuildStarted {
                slice_name: "identity-user-registration".to_string(),
            },
            &[r#""event":"slice.build.started""#, r#""slice-name":"identity-user-registration""#],
        ),
        (
            EventKind::SliceBuildSucceeded {
                slice_name: "identity-user-registration".to_string(),
            },
            &[r#""event":"slice.build.succeeded""#, r#""slice-name":"identity-user-registration""#],
        ),
        (
            EventKind::SliceBuildFailed {
                slice_name: "identity-user-registration".to_string(),
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
                slice_name: "identity-user-registration".to_string(),
            },
            &[r#""event":"slice.merge.started""#, r#""slice-name":"identity-user-registration""#],
        ),
        (
            EventKind::SliceMergeSucceeded {
                slice_name: "identity-user-registration".to_string(),
            },
            &[r#""event":"slice.merge.succeeded""#, r#""slice-name":"identity-user-registration""#],
        ),
        (
            EventKind::SliceMergeFailed {
                slice_name: "identity-user-registration".to_string(),
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
                slice: "identity-user-registration".to_string(),
                target: "omnia".to_string(),
            },
            &[
                r#""event":"target.execution.agent""#,
                r#""slice":"identity-user-registration""#,
                r#""target":"omnia""#,
            ],
        ),
    ];

    for (kind, required) in rows {
        let event = Event::new(test_timestamp("2026-05-22T13:15:00Z"), kind.clone());
        let json = serde_json::to_string(&event).expect("serialise build/merge event");
        for needle in *required {
            assert!(json.contains(needle), "wire form must contain `{needle}`; got:\n{json}");
        }
        let round: Event = serde_json::from_str(&json).expect("deserialise build/merge event");
        assert_eq!(round, event, "build/merge round-trip must preserve every field");
    }
}

#[test]
fn slice_synthesize_completed_omits_empty_artifacts() {
    // `artifacts` carries `skip_serializing_if = "Vec::is_empty"`
    // so an empty list does not reach the wire at all.
    let event = Event::new(
        test_timestamp("2026-05-22T13:15:00Z"),
        EventKind::SliceSynthesizeCompleted {
            slice_name: "identity-user-registration".to_string(),
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

    // Guard against an accidental rename_all = "kebab-case" on the
    // payload structs — those would flip the snake_case fields to
    // hyphenated names and silently break the RFC example.
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
#[expect(
    clippy::too_many_lines,
    reason = "Single sweep covers every payload-bearing variant; splitting hides the wire-format coverage discipline."
)]
fn no_snake_case_leaks_to_wire() {
    // workflow §Wire format: snake_case lifecycle values are never
    // produced on disk. Exercise every variant that carries an
    // enum-shaped or hyphenable field name.
    let dir = tempdir().expect("tempdir");
    let layout = Layout::new(dir.path());
    for kind in [
        EventKind::PlanTransitionApproved {
            plan_name: "p".to_string(),
        },
        EventKind::PlanAmendDivergence {
            plan_name: "p".to_string(),
            slice_name: "s".to_string(),
            from: Divergence::None,
            to: Divergence::Accepted,
        },
        EventKind::SliceTransitionRefined {
            slice_name: "s".to_string(),
        },
        EventKind::SliceSynthesizeStarted {
            slice_name: "s".to_string(),
        },
        EventKind::SliceSynthesizeAgent {
            slice_name: "s".to_string(),
        },
        EventKind::SliceSynthesizeCompleted {
            slice_name: "s".to_string(),
            artifacts: vec!["proposal.md".to_string()],
        },
        EventKind::SliceSynthesizeFailed {
            slice_name: "s".to_string(),
            reason: "spec-requirement-missing-sources".to_string(),
        },
        EventKind::SliceBuildStarted {
            slice_name: "s".to_string(),
        },
        EventKind::SliceBuildSucceeded {
            slice_name: "s".to_string(),
        },
        EventKind::SliceBuildFailed {
            slice_name: "s".to_string(),
            reason: "cargo-check-failed".to_string(),
        },
        EventKind::SliceMergeStarted {
            slice_name: "s".to_string(),
        },
        EventKind::SliceMergeSucceeded {
            slice_name: "s".to_string(),
        },
        EventKind::SliceMergeFailed {
            slice_name: "s".to_string(),
            reason: "baseline-conflict".to_string(),
        },
        EventKind::TargetExecutionAgent {
            slice: "s".to_string(),
            target: "omnia".to_string(),
        },
        EventKind::SliceExtractCompleted {
            slice_name: "s".to_string(),
            source: "k".to_string(),
        },
        EventKind::SourceSurveyCacheHit {
            source: "k".to_string(),
            adapter: "captures".to_string(),
            fingerprint: "sha256:beef".to_string(),
        },
        EventKind::SourceSurveyCacheMiss {
            source: "k".to_string(),
            adapter: "captures".to_string(),
            fingerprint: "sha256:beef".to_string(),
            reason: CacheMissReason::AdapterOptOut,
        },
        EventKind::SourceExecutionAgent {
            source: "k".to_string(),
            adapter: "captures".to_string(),
            operation: SourceOperation::Extract,
        },
        EventKind::PlanReconcileCompleted {
            plan_name: "p".to_string(),
            slice_count: 1,
            slice_names: vec!["s".to_string()],
        },
        EventKind::SliceArchiveCreated {
            slice_name: "s".to_string(),
            touched_specs: vec!["identity".to_string()],
            outcome_summary: "identity: 1 modified".to_string(),
            merge_sha: Some("abc1234".to_string()),
        },
    ] {
        append_batch(
            layout,
            std::slice::from_ref(&Event::new(test_timestamp("2026-05-21T20:05:00Z"), kind)),
        )
        .expect("append ok");
    }
    let raw = std::fs::read_to_string(path(layout)).expect("read journal");
    for needle in [
        "plan_name",
        "slice_name",
        "slice_count",
        "slice_names",
        "requirement_id",
        "in_progress",
        "touched_specs",
        "outcome_summary",
        "merge_sha",
    ] {
        assert!(
            !raw.contains(needle),
            "snake_case `{needle}` must not appear on the wire; raw:\n{raw}"
        );
    }
}
