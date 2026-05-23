//! RFC-25 journal events.
//!
//! Append-only newline-delimited JSON at `.specify/journal.jsonl`,
//! shared by every plan-, slice-, propose-, extract-, and synthesis-
//! related signal listed in [RFC-25 §Observability]. One line per
//! [`Event`]; readers tail the file and skip blank lines.
//!
//! Wire format is locked: event ids are dotted kebab-case
//! (`plan.transition.reviewed`), payload field names are kebab-case
//! (`plan-name`, `slice-name`, …), and the closed `from` / `to`
//! enum is `none | likely | accepted | rejected`. Rust variant
//! names stay `snake_case` and reach the wire through
//! `#[serde(rename = "…")]`.
//!
//! [RFC-25 §Observability]: https://github.com/augentic/specify/blob/main/rfcs/rfc-25-workflow.md#observability-rfc-19

use std::io::Write;
use std::path::PathBuf;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use specify_error::Error;

use crate::change::Divergence;
use crate::config::Layout;

/// Project-relative path the journal lives at.
const JOURNAL_FILE_NAME: &str = "journal.jsonl";

/// One row of the journal. Serialises as `{ timestamp, event,
/// payload }` — RFC-25 §Wire format pins `timestamp` first so a
/// `head -1` on the file is enough to confirm the run window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    /// Second-precision UTC timestamp (`%Y-%m-%dT%H:%M:%SZ`).
    #[serde(with = "specify_error::serde_rfc3339")]
    pub timestamp: Timestamp,
    /// Event id + payload, adjacently tagged so `event` and `payload`
    /// sit side by side in the JSON object.
    #[serde(flatten)]
    pub kind: EventKind,
}

impl Event {
    /// Build an [`Event`] at `timestamp` carrying `kind`. Tests pin
    /// the timestamp; production callers pass `Timestamp::now()`.
    #[must_use]
    pub const fn new(timestamp: Timestamp, kind: EventKind) -> Self {
        Self { timestamp, kind }
    }
}

/// The RFC-25 §Observability event set.
///
/// Adjacently-tagged on the wire as `{ event: <id>, payload: {…} }`
/// so the dotted-kebab-case event id is a top-level field consumers
/// can filter on without parsing the payload first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", content = "payload")]
pub enum EventKind {
    /// Gate 1 cleared — `specify plan transition <plan-name> reviewed`.
    #[serde(rename = "plan.transition.reviewed", rename_all = "kebab-case")]
    PlanTransitionReviewed {
        /// Plan name from `plan.yaml.name`.
        plan_name: String,
    },
    /// `/spec:plan`'s `propose` sub-step flagged a materially-
    /// disagreeing slice (`slices[].divergence: likely`).
    /// RFC-27 §D5 — emitted from the CLI when the operator (or the
    /// `plan` skill body) runs `specify plan create
    /// --divergence-likely <slice>` or `specify plan amend
    /// --divergence likely`; the skill is no longer the writer.
    #[serde(rename = "plan.propose.divergence", rename_all = "kebab-case")]
    PlanProposeDivergence {
        /// Plan name from `plan.yaml.name`.
        plan_name: String,
        /// Slice id under `plan.yaml.slices[].name`.
        slice_name: String,
    },
    /// Operator stamped `slices[].divergence` via
    /// `specify plan amend --divergence <likely|accepted|rejected>`.
    /// RFC-27 §D5 — the CLI is the single writer; `likely` reaches
    /// this event from skill-body fallbacks against existing
    /// `plan.yaml` entries (the post-`propose` happy path stages
    /// `likely` via `specify plan create --divergence-likely`, which
    /// emits [`Self::PlanProposeDivergence`] instead).
    #[serde(rename = "plan.amend.divergence", rename_all = "kebab-case")]
    PlanAmendDivergence {
        /// Plan name from `plan.yaml.name`.
        plan_name: String,
        /// Slice id under `plan.yaml.slices[].name`.
        slice_name: String,
        /// Previous value — may be any of `none | likely | accepted | rejected`.
        /// Callers convert an absent on-disk slice field via
        /// `previous.unwrap_or(Divergence::None)`.
        from: Divergence,
        /// New value — `likely`, `accepted`, or `rejected`. The
        /// implicit `none` default is rejected at the flag-parser
        /// level; omit `--divergence` to leave the field unchanged.
        to: Divergence,
    },
    /// Slice transitioned to `refined` — synthesis finished and the
    /// slice is ready for `/spec:build`.
    #[serde(rename = "slice.transition.refined", rename_all = "kebab-case")]
    SliceTransitionRefined {
        /// Slice id under `plan.yaml.slices[].name`.
        slice_name: String,
    },
    /// `/spec:refine` finished one source-bound `extract` call. One
    /// event per `(source-key, slice)` pair. Agent-driven.
    #[serde(rename = "slice.extract.completed", rename_all = "kebab-case")]
    SliceExtractCompleted {
        /// Slice id under `plan.yaml.slices[].name`.
        slice_name: String,
        /// Source key from `plan.yaml.sources.<key>`.
        source_key: String,
    },
    /// Synthesis wrote `[conflict]` on a requirement in `spec.md` —
    /// same-authority disagreement that the operator must reconcile.
    /// Agent-driven.
    #[serde(rename = "slice.synthesis.conflict", rename_all = "kebab-case")]
    SliceSynthesisConflict {
        /// Slice id under `plan.yaml.slices[].name`.
        slice_name: String,
        /// `ID:` value on the tagged requirement block.
        requirement_id: String,
    },
    /// Synthesis wrote `[divergence]` on a requirement in `spec.md` —
    /// cross-authority disagreement preserved as inline commentary.
    /// Agent-driven.
    #[serde(rename = "slice.synthesis.divergence", rename_all = "kebab-case")]
    SliceSynthesisDivergence {
        /// Slice id under `plan.yaml.slices[].name`.
        slice_name: String,
        /// `ID:` value on the tagged requirement block.
        requirement_id: String,
    },
    /// Synthesis wrote `[unknown]` on a requirement in `spec.md` — a
    /// gap the operator must close before the requirement is
    /// meaningful. Agent-driven.
    #[serde(rename = "slice.synthesis.unknown", rename_all = "kebab-case")]
    SliceSynthesisUnknown {
        /// Slice id under `plan.yaml.slices[].name`.
        slice_name: String,
        /// `ID:` value on the tagged requirement block.
        requirement_id: String,
    },
    /// RFC-27 §D8 — cache lookup matched and `extract` was *not*
    /// re-run. CI pinning the five fingerprint inputs at a known set
    /// can re-run any prior `/spec:execute` and expect byte-stable
    /// cache hits.
    #[serde(rename = "slice.extract.cache-hit", rename_all = "kebab-case")]
    SliceExtractCacheHit {
        /// Slice id under `plan.yaml.slices[].name`.
        slice_name: String,
        /// Source key from `plan.yaml.sources.<key>`.
        source_key: String,
        /// Adapter name (kebab-case; mirrors `adapter.yaml.name`).
        adapter: String,
        /// sha256 hex digest of the [`crate::adapter::CacheFingerprint`]
        /// inputs the cache layer keyed against.
        fingerprint: String,
    },
    /// RFC-27 §D8 — cache lookup missed and `extract` ran. `reason`
    /// is one of the closed [`CacheMissReason`] values; CI observing
    /// any of them knows exactly which input drifted.
    #[serde(rename = "slice.extract.cache-miss", rename_all = "kebab-case")]
    SliceExtractCacheMiss {
        /// Slice id under `plan.yaml.slices[].name`.
        slice_name: String,
        /// Source key from `plan.yaml.sources.<key>`.
        source_key: String,
        /// Adapter name (kebab-case; mirrors `adapter.yaml.name`).
        adapter: String,
        /// sha256 hex digest of the [`crate::adapter::CacheFingerprint`]
        /// inputs the cache layer computed for this run.
        fingerprint: String,
        /// Which fingerprint input drifted (or `no-prior-entry` on
        /// first sight; `adapter-opt-out` when the adapter declared
        /// `cache: opt-out`).
        reason: CacheMissReason,
    },
    /// RFC-27 §D4 — `/spec:refine` wrote `fusion.yaml` for a slice.
    /// Agent-driven from `/spec:refine` step 5.
    #[serde(rename = "slice.fusion.written", rename_all = "kebab-case")]
    SliceFusionWritten {
        /// Slice id under `plan.yaml.slices[].name`.
        slice_name: String,
        /// CLI version that authored the file (e.g. `specify@2.1.0`).
        generator: String,
        /// Count of `requirements[]` rows written.
        requirement_count: usize,
    },
    /// RFC-27 §D1 — target's `build` finished fixture replay.
    /// Payload mirrors the `fixture-replay:` block written into the
    /// slice's `.metadata.yaml`. Optional in v1 (targets that have
    /// not implemented the hook do not emit this event).
    #[serde(rename = "slice.fixture-replay.completed", rename_all = "kebab-case")]
    SliceFixtureReplayCompleted {
        /// Slice id under `plan.yaml.slices[].name`.
        slice_name: String,
        /// Fixture-runner identity (e.g. `omnia-target@1.4 (cargo nextest)`).
        runner: String,
        /// Number of fixtures that passed replay.
        passed: usize,
        /// Number of fixtures that failed replay.
        failed: usize,
        /// Number of fixtures the runner skipped.
        skipped: usize,
    },
    /// RFC-27 §D3 — operator set or cleared a per-slice
    /// `authority-override` map at Gate 1. CLI-driven via
    /// `specify plan create --authority-override`,
    /// `specify plan amend --authority-override`, or the matching
    /// `--clear-*` flags.
    #[serde(rename = "plan.amend.authority-override", rename_all = "kebab-case")]
    PlanAmendAuthorityOverride {
        /// Plan name from `plan.yaml.name`.
        plan_name: String,
        /// Slice id under `plan.yaml.slices[].name`.
        slice_name: String,
        /// Closed action discriminator.
        action: AuthorityOverrideAction,
        /// Claim kind the action touched (the closed-enum key under
        /// `slices[].authority-override`). `None` only when `action`
        /// is [`AuthorityOverrideAction::ClearAll`].
        #[serde(default, skip_serializing_if = "Option::is_none")]
        claim_kind: Option<String>,
        /// Source key the override now points at, when `action` is
        /// [`AuthorityOverrideAction::Set`]; absent on clear actions.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source_key: Option<String>,
    },
}

/// Closed `reason` enum on [`EventKind::SliceExtractCacheMiss`].
///
/// Each value names one of the five fingerprint inputs from RFC-27
/// §D8 (plus `no-prior-entry` for first runs and `adapter-opt-out`
/// for `cache: opt-out` adapters). Operators reading `index.jsonl`
/// see exactly which input drifted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, strum::Display)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum CacheMissReason {
    /// No prior cache entry — first run for this fingerprint key.
    NoPriorEntry,
    /// `$SOURCE_DIR` canonical path (or value-binding sha256) changed.
    SourcePathChanged,
    /// `<name>@<version>` from `adapter.yaml` changed.
    AdapterVersionChanged,
    /// sha256 of the brief markdown driving the operation changed.
    BriefShaChanged,
    /// One of the declared-tool versions changed.
    ToolVersionChanged,
    /// The adapter declared `cache: opt-out`; the CLI bypasses the
    /// cache and the matching journal event carries this reason.
    AdapterOptOut,
}

/// Closed `action` enum on [`EventKind::PlanAmendAuthorityOverride`].
///
/// Mirrors the CLI surface (`--authority-override`,
/// `--clear-authority-override`, `--clear-authority-overrides`) so
/// operators reading the journal see which flag drove each row.
/// `Ord` is derived so batched-append callers can sort
/// `(slice, kind, action)` keys for byte-stable journal output —
/// the declaration order (`Set < Clear < ClearAll`) is also the
/// dispatch order in `mutate_authority_overrides`, so a stable
/// alphabetical-by-action sort happens to match operator intent
/// out of the box.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, strum::Display,
)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum AuthorityOverrideAction {
    /// `--authority-override <slice> <kind>=<key>` set the value.
    Set,
    /// `--clear-authority-override <slice> <kind>` removed one entry.
    Clear,
    /// `--clear-authority-overrides <slice>` removed every entry.
    ClearAll,
}

/// Absolute path to the journal at `<project_dir>/.specify/journal.jsonl`.
#[must_use]
pub fn path(layout: Layout<'_>) -> PathBuf {
    layout.specify_dir().join(JOURNAL_FILE_NAME)
}

/// Append a sequence of [`Event`]s to the project journal in a
/// single write call.
///
/// Opens `<project_dir>/.specify/journal.jsonl` in append mode,
/// creating the file (and the `.specify/` directory) on first
/// write. All events are serialised, concatenated as
/// newline-terminated JSON lines, and pushed through one
/// `write_all` followed by one `sync_all`. Either every line
/// lands on disk or none does — downstream consumers never
/// observe a partial-state batch. A POSIX `O_APPEND` write of
/// ≤ `PIPE_BUF` bytes is atomic against concurrent writers on
/// local filesystems, which is the safety envelope a workflow
/// journal needs — RFC-25 emits one event per CLI verb
/// invocation, well below the limit.
///
/// Used by CLI verbs that own more than one journal emit per
/// invocation (e.g. `specify plan create --auto-review`, which
/// stages both `plan.propose.divergence` and
/// `plan.transition.reviewed` in the same Gate-1 consent), and
/// equally by single-event callers via
/// `append_batch(layout, std::slice::from_ref(&event))`.
///
/// Empty `events` is a no-op; the journal file is not created on
/// disk and `Ok(())` is returned. This lets callers compose the
/// batch unconditionally (collecting events into a `Vec` and
/// passing the slice in) without an outer `is_empty` check.
///
/// # Errors
///
/// Propagates I/O failures from the directory create / open /
/// write / fsync chain, plus JSON serialisation failures as
/// `journal-event-serialise-failed`.
pub fn append_batch(layout: Layout<'_>, events: &[Event]) -> Result<(), Error> {
    if events.is_empty() {
        return Ok(());
    }
    std::fs::create_dir_all(layout.specify_dir())?;
    let path = path(layout);
    let mut payload = String::new();
    for event in events {
        let line = serde_json::to_string(event).map_err(|err| Error::Diag {
            code: "journal-event-serialise-failed",
            detail: format!("failed to serialise journal event: {err}"),
        })?;
        payload.push_str(&line);
        payload.push('\n');
    }
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(&path)?;
    file.write_all(payload.as_bytes())?;
    file.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    fn ts(raw: &str) -> Timestamp {
        raw.parse().expect("valid rfc3339 timestamp in test fixture")
    }

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
            ts("2026-05-21T20:02:00Z"),
            EventKind::SliceTransitionRefined {
                slice_name: "checkout".to_string(),
            },
        );
        append_batch(layout, std::slice::from_ref(&event)).expect("append ok");

        assert!(layout.specify_dir().is_dir(), ".specify/ must exist after first append");
        assert!(path(layout).is_file(), "journal.jsonl must exist after first append");
    }

    #[test]
    fn append_batch_writes_every_event_in_order_in_one_call() {
        // RFC-27 §D7: `specify plan create --auto-review` may emit
        // both `plan.propose.divergence` and
        // `plan.transition.reviewed` in a single fsynced append.
        // Exercise the batched helper to lock that contract.
        let dir = tempdir().expect("tempdir");
        let layout = Layout::new(dir.path());
        let events = vec![
            Event::new(
                ts("2026-05-22T13:30:00Z"),
                EventKind::PlanProposeDivergence {
                    plan_name: "fresh".to_string(),
                    slice_name: "checkout".to_string(),
                },
            ),
            Event::new(
                ts("2026-05-22T13:30:00Z"),
                EventKind::PlanTransitionReviewed {
                    plan_name: "fresh".to_string(),
                },
            ),
        ];
        append_batch(layout, &events).expect("append_batch ok");

        let lines = read_lines(layout);
        assert_eq!(lines.len(), 2, "expected two journal lines, got {}", lines.len());
        assert!(
            lines[0].contains(r#""event":"plan.propose.divergence""#),
            "first line must be plan.propose.divergence, got:\n{}",
            lines[0]
        );
        assert!(
            lines[1].contains(r#""event":"plan.transition.reviewed""#),
            "second line must be plan.transition.reviewed, got:\n{}",
            lines[1]
        );
    }

    #[test]
    fn append_batch_empty_slice_is_no_op() {
        // Callers (e.g. `plan create` without `--auto-review` and
        // without `--divergence-likely`) build the batch
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
    fn slice_extract_cache_hit_wire_shape() {
        let dir = tempdir().expect("tempdir");
        let layout = Layout::new(dir.path());
        let event = Event::new(
            ts("2026-05-22T13:15:00Z"),
            EventKind::SliceExtractCacheHit {
                slice_name: "identity-user-registration".to_string(),
                source_key: "runtime".to_string(),
                adapter: "code-runtime".to_string(),
                fingerprint: "sha256:cafef00d".to_string(),
            },
        );
        append_batch(layout, std::slice::from_ref(&event)).expect("append ok");
        let lines = read_lines(layout);
        assert_eq!(lines.len(), 1);
        assert_eq!(
            lines[0],
            r#"{"timestamp":"2026-05-22T13:15:00Z","event":"slice.extract.cache-hit","payload":{"slice-name":"identity-user-registration","source-key":"runtime","adapter":"code-runtime","fingerprint":"sha256:cafef00d"}}"#
        );
    }

    #[test]
    fn slice_extract_cache_miss_wire_shape() {
        let dir = tempdir().expect("tempdir");
        let layout = Layout::new(dir.path());
        let event = Event::new(
            ts("2026-05-22T13:15:01Z"),
            EventKind::SliceExtractCacheMiss {
                slice_name: "identity-user-registration".to_string(),
                source_key: "runtime".to_string(),
                adapter: "code-runtime".to_string(),
                fingerprint: "sha256:beef".to_string(),
                reason: CacheMissReason::AdapterVersionChanged,
            },
        );
        append_batch(layout, std::slice::from_ref(&event)).expect("append ok");
        let line = read_lines(layout).pop().expect("at least one line");
        assert!(line.contains(r#""event":"slice.extract.cache-miss""#));
        assert!(line.contains(r#""reason":"adapter-version-changed""#));
        assert!(line.contains(r#""source-key":"runtime""#));
    }

    #[test]
    fn cache_miss_reason_round_trips_kebab_case() {
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
    fn slice_fusion_written_wire_shape() {
        let dir = tempdir().expect("tempdir");
        let layout = Layout::new(dir.path());
        let event = Event::new(
            ts("2026-05-22T13:16:00Z"),
            EventKind::SliceFusionWritten {
                slice_name: "identity-user-registration".to_string(),
                generator: "specify@2.1.0".to_string(),
                requirement_count: 7,
            },
        );
        append_batch(layout, std::slice::from_ref(&event)).expect("append ok");
        let line = read_lines(layout).pop().expect("line");
        assert!(line.contains(r#""event":"slice.fusion.written""#));
        assert!(line.contains(r#""generator":"specify@2.1.0""#));
        assert!(line.contains(r#""requirement-count":7"#));
    }

    #[test]
    fn slice_fixture_replay_completed_wire_shape() {
        let dir = tempdir().expect("tempdir");
        let layout = Layout::new(dir.path());
        let event = Event::new(
            ts("2026-05-22T13:18:42Z"),
            EventKind::SliceFixtureReplayCompleted {
                slice_name: "identity-user-registration".to_string(),
                runner: "omnia-target@1.4 (cargo nextest)".to_string(),
                passed: 47,
                failed: 0,
                skipped: 0,
            },
        );
        append_batch(layout, std::slice::from_ref(&event)).expect("append ok");
        let line = read_lines(layout).pop().expect("line");
        assert!(line.contains(r#""event":"slice.fixture-replay.completed""#));
        assert!(line.contains(r#""passed":47"#));
        assert!(line.contains(r#""failed":0"#));
        assert!(line.contains(r#""skipped":0"#));
        assert!(line.contains(r#""runner":"omnia-target@1.4 (cargo nextest)""#));
    }

    #[test]
    fn plan_amend_authority_override_wire_shape() {
        let dir = tempdir().expect("tempdir");
        let layout = Layout::new(dir.path());
        let event = Event::new(
            ts("2026-05-22T13:20:00Z"),
            EventKind::PlanAmendAuthorityOverride {
                plan_name: "identity-revamp".to_string(),
                slice_name: "identity-user-registration".to_string(),
                action: AuthorityOverrideAction::Set,
                claim_kind: Some("criterion".to_string()),
                source_key: Some("runtime".to_string()),
            },
        );
        append_batch(layout, std::slice::from_ref(&event)).expect("append ok");
        let line = read_lines(layout).pop().expect("line");
        assert!(line.contains(r#""event":"plan.amend.authority-override""#));
        assert!(line.contains(r#""action":"set""#));
        assert!(line.contains(r#""claim-kind":"criterion""#));
        assert!(line.contains(r#""source-key":"runtime""#));
    }

    #[test]
    fn plan_amend_authority_override_clear_all_elides_optional_fields() {
        let dir = tempdir().expect("tempdir");
        let layout = Layout::new(dir.path());
        let event = Event::new(
            ts("2026-05-22T13:20:01Z"),
            EventKind::PlanAmendAuthorityOverride {
                plan_name: "identity-revamp".to_string(),
                slice_name: "identity-user-registration".to_string(),
                action: AuthorityOverrideAction::ClearAll,
                claim_kind: None,
                source_key: None,
            },
        );
        append_batch(layout, std::slice::from_ref(&event)).expect("append ok");
        let line = read_lines(layout).pop().expect("line");
        assert!(line.contains(r#""action":"clear-all""#));
        assert!(!line.contains("claim-kind"), "absent claim-kind must elide, got:\n{line}");
        assert!(!line.contains("source-key"), "absent source-key must elide, got:\n{line}");
    }

    #[test]
    fn no_snake_case_fields_or_values_leak_to_wire() {
        // RFC-25 §Wire format: snake_case lifecycle values are never
        // produced on disk. Exercise every variant that carries an
        // enum-shaped or hyphenable field name.
        let dir = tempdir().expect("tempdir");
        let layout = Layout::new(dir.path());
        for kind in [
            EventKind::PlanTransitionReviewed {
                plan_name: "p".to_string(),
            },
            EventKind::PlanProposeDivergence {
                plan_name: "p".to_string(),
                slice_name: "s".to_string(),
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
            EventKind::SliceExtractCompleted {
                slice_name: "s".to_string(),
                source_key: "k".to_string(),
            },
        ] {
            append_batch(
                layout,
                std::slice::from_ref(&Event::new(ts("2026-05-21T20:05:00Z"), kind)),
            )
            .expect("append ok");
        }
        let raw = std::fs::read_to_string(path(layout)).expect("read journal");
        for needle in ["plan_name", "slice_name", "source_key", "requirement_id", "in_progress"] {
            assert!(
                !raw.contains(needle),
                "snake_case `{needle}` must not appear on the wire; raw:\n{raw}"
            );
        }
    }
}
