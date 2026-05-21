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
#[non_exhaustive]
pub enum EventKind {
    /// Gate 1 cleared — `specify plan transition <plan-name> reviewed`.
    #[serde(rename = "plan.transition.reviewed", rename_all = "kebab-case")]
    PlanTransitionReviewed {
        /// Plan name from `plan.yaml.name`.
        plan_name: String,
    },
    /// `/spec:plan`'s `propose` sub-step flagged a materially-
    /// disagreeing slice (`slices[].divergence: likely`).
    /// Agent-driven: emitted via [`append`] from the skill body.
    #[serde(rename = "plan.propose.divergence", rename_all = "kebab-case")]
    PlanProposeDivergence {
        /// Plan name from `plan.yaml.name`.
        plan_name: String,
        /// Slice id under `plan.yaml.slices[].name`.
        slice_name: String,
    },
    /// Operator stamped `slices[].divergence` at Gate 1 via
    /// `specify plan amend --divergence <accepted|rejected>`.
    #[serde(rename = "plan.amend.divergence", rename_all = "kebab-case")]
    PlanAmendDivergence {
        /// Plan name from `plan.yaml.name`.
        plan_name: String,
        /// Slice id under `plan.yaml.slices[].name`.
        slice_name: String,
        /// Previous value — may be any of `none | likely | accepted | rejected`.
        from: DivergenceState,
        /// New value — `accepted` or `rejected` (the only operator-
        /// settable values; `likely` is propose-only, `none` is the
        /// implicit default and is never set explicitly).
        to: DivergenceState,
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
}

/// Closed `none | likely | accepted | rejected` enum used by the
/// `plan.amend.divergence` payload's `from` / `to` fields.
///
/// Distinct from [`crate::change::Divergence`] (which has no `None`
/// variant because absence on disk encodes the same meaning) so the
/// wire shape can express the implicit-default first transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, strum::Display)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
#[non_exhaustive]
pub enum DivergenceState {
    /// No divergence — the implicit default; absent on disk.
    None,
    /// `propose` flagged the slice as materially divergent.
    Likely,
    /// Operator acknowledged the divergence at Gate 1.
    Accepted,
    /// Operator rejected the divergence at Gate 1.
    Rejected,
}

impl From<Option<crate::change::Divergence>> for DivergenceState {
    fn from(value: Option<crate::change::Divergence>) -> Self {
        match value {
            None => Self::None,
            Some(crate::change::Divergence::Likely) => Self::Likely,
            Some(crate::change::Divergence::Accepted) => Self::Accepted,
            Some(crate::change::Divergence::Rejected) => Self::Rejected,
        }
    }
}

/// Absolute path to the journal at `<project_dir>/.specify/journal.jsonl`.
#[must_use]
pub fn path(layout: Layout<'_>) -> PathBuf {
    layout.specify_dir().join(JOURNAL_FILE_NAME)
}

/// Append one [`Event`] to the project journal.
///
/// Opens `<project_dir>/.specify/journal.jsonl` in append mode,
/// creating the file (and the `.specify/` directory) on first
/// write, and emits the event as a single JSON line followed by
/// `\n`. A POSIX `O_APPEND` write of ≤ `PIPE_BUF` bytes is atomic
/// against concurrent writers on local filesystems, which is the
/// safety envelope a workflow journal needs — RFC-25 emits one
/// event per CLI verb invocation, well below the limit.
///
/// # Errors
///
/// Propagates I/O failures from the directory create / open /
/// write / fsync chain.
///
/// # Panics
///
/// Panics if [`serde_json::to_string`] fails for [`Event`]. Every
/// variant is a closed serde derive whose fields are owned `String`s
/// or [`DivergenceState`] (a flat enum); this branch is unreachable
/// in normal operation and mirrors the `to_value(entry).expect("plan
/// Entry serialises as JSON")` pattern in `src/commands/plan/create.rs`.
pub fn append(layout: Layout<'_>, event: &Event) -> Result<(), Error> {
    std::fs::create_dir_all(layout.specify_dir())?;
    let path = path(layout);
    let line = serde_json::to_string(event).expect("Event serialises as JSON");
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(&path)?;
    writeln!(file, "{line}")?;
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
    fn plan_transition_reviewed_wire_shape() {
        let dir = tempdir().expect("tempdir");
        let layout = Layout::new(dir.path());
        let event = Event::new(
            ts("2026-05-21T20:00:00Z"),
            EventKind::PlanTransitionReviewed {
                plan_name: "platform-v2".to_string(),
            },
        );
        append(layout, &event).expect("append ok");

        let lines = read_lines(layout);
        assert_eq!(lines.len(), 1);
        assert_eq!(
            lines[0],
            r#"{"timestamp":"2026-05-21T20:00:00Z","event":"plan.transition.reviewed","payload":{"plan-name":"platform-v2"}}"#
        );
    }

    #[test]
    fn plan_amend_divergence_wire_shape() {
        let dir = tempdir().expect("tempdir");
        let layout = Layout::new(dir.path());
        let event = Event::new(
            ts("2026-05-21T20:01:00Z"),
            EventKind::PlanAmendDivergence {
                plan_name: "platform-v2".to_string(),
                slice_name: "checkout".to_string(),
                from: DivergenceState::Likely,
                to: DivergenceState::Accepted,
            },
        );
        append(layout, &event).expect("append ok");

        let lines = read_lines(layout);
        assert_eq!(lines.len(), 1);
        assert!(
            lines[0].contains(r#""event":"plan.amend.divergence""#),
            "missing event id in line:\n{}",
            lines[0]
        );
        assert!(
            lines[0].contains(r#""from":"likely""#),
            "from must serialise kebab-case `likely`, got:\n{}",
            lines[0]
        );
        assert!(
            lines[0].contains(r#""to":"accepted""#),
            "to must serialise kebab-case `accepted`, got:\n{}",
            lines[0]
        );
        assert!(
            lines[0].contains(r#""plan-name":"platform-v2""#),
            "plan-name must be kebab-case, got:\n{}",
            lines[0]
        );
        assert!(
            lines[0].contains(r#""slice-name":"checkout""#),
            "slice-name must be kebab-case, got:\n{}",
            lines[0]
        );
    }

    #[test]
    fn divergence_state_from_option_divergence_round_trip() {
        use crate::change::Divergence;
        assert_eq!(DivergenceState::from(None), DivergenceState::None);
        assert_eq!(DivergenceState::from(Some(Divergence::Likely)), DivergenceState::Likely);
        assert_eq!(DivergenceState::from(Some(Divergence::Accepted)), DivergenceState::Accepted);
        assert_eq!(DivergenceState::from(Some(Divergence::Rejected)), DivergenceState::Rejected);
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
        append(layout, &event).expect("append ok");

        assert!(layout.specify_dir().is_dir(), ".specify/ must exist after first append");
        assert!(path(layout).is_file(), "journal.jsonl must exist after first append");
    }

    #[test]
    fn appending_two_events_writes_two_lines_in_order() {
        let dir = tempdir().expect("tempdir");
        let layout = Layout::new(dir.path());
        let first = Event::new(
            ts("2026-05-21T20:03:00Z"),
            EventKind::SliceExtractCompleted {
                slice_name: "checkout".to_string(),
                source_key: "monolith".to_string(),
            },
        );
        let second = Event::new(
            ts("2026-05-21T20:03:01Z"),
            EventKind::SliceSynthesisConflict {
                slice_name: "checkout".to_string(),
                requirement_id: "R-01".to_string(),
            },
        );
        append(layout, &first).expect("append first");
        append(layout, &second).expect("append second");

        let lines = read_lines(layout);
        assert_eq!(lines.len(), 2, "expected two journal lines, got {}", lines.len());
        assert!(lines[0].contains(r#""event":"slice.extract.completed""#));
        assert!(lines[1].contains(r#""event":"slice.synthesis.conflict""#));
    }

    #[test]
    fn all_synthesis_tag_variants_serialise() {
        // Locks the wire shape for the three `slice.synthesis.*` events
        // that W3.1 will emit from the synthesis pipeline.
        let dir = tempdir().expect("tempdir");
        let layout = Layout::new(dir.path());
        for (kind, expected_event) in [
            (
                EventKind::SliceSynthesisConflict {
                    slice_name: "checkout".to_string(),
                    requirement_id: "R-01".to_string(),
                },
                "slice.synthesis.conflict",
            ),
            (
                EventKind::SliceSynthesisDivergence {
                    slice_name: "checkout".to_string(),
                    requirement_id: "R-02".to_string(),
                },
                "slice.synthesis.divergence",
            ),
            (
                EventKind::SliceSynthesisUnknown {
                    slice_name: "checkout".to_string(),
                    requirement_id: "R-03".to_string(),
                },
                "slice.synthesis.unknown",
            ),
        ] {
            append(layout, &Event::new(ts("2026-05-21T20:04:00Z"), kind)).expect("append ok");
            let lines = read_lines(layout);
            let last = lines.last().expect("at least one line");
            assert!(
                last.contains(&format!(r#""event":"{expected_event}""#)),
                "missing {expected_event} in:\n{last}"
            );
            assert!(
                last.contains(r#""requirement-id":"#),
                "requirement-id must be kebab-case, got:\n{last}"
            );
        }
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
                from: DivergenceState::None,
                to: DivergenceState::Accepted,
            },
            EventKind::SliceTransitionRefined {
                slice_name: "s".to_string(),
            },
            EventKind::SliceExtractCompleted {
                slice_name: "s".to_string(),
                source_key: "k".to_string(),
            },
        ] {
            append(layout, &Event::new(ts("2026-05-21T20:05:00Z"), kind)).expect("append ok");
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
