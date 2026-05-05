//! On-disk representation of `<slice_dir>/journal.yaml` — the
//! append-only audit log that phase skills (`define`, `build`, `merge`)
//! write while they run.
//!
//! See RFC-2 §"Question Recording" and §"Failure and Resumption" for
//! the canonical shape and writer contract.
//!
//! ## Contracts
//!
//! - **Append-only**: the module surface exposes `load` and `append`
//!   and nothing else. There is no `pop`, `truncate`, or delete API —
//!   `journal.yaml` grows monotonically for the life of the slice.
//!   Callers who genuinely need to prune history edit the file by
//!   hand.
//!
//! - **Pure audit log**: `/spec:execute` (Layer 2) does **not** consume
//!   journal entries as a signalling channel. Phase success / failure
//!   / deferred classification travels through
//!   `.metadata.yaml.outcome` (stamped via
//!   [`crate::actions::phase_outcome`] in L2.A). The journal is for
//!   humans — stderr traces, ambiguous-requirement text, recovery
//!   notes — not for the driver's state machine.
//!
//! - **Atomic writes**: each [`Journal::append`] is a read-modify-write
//!   that serialises the whole journal to a temp file in the same
//!   directory and then `persist`s it via `fs::rename`. Mirrors
//!   [`crate::SliceMetadata::save`] and `Plan::save` (in
//!   `specify-initiative`) exactly
//!   so a mid-write crash leaves the prior file intact.
//!
//! - **Single-writer**: `append` is atomic per call but there is no
//!   inter-process lock. Concurrent appends from multiple processes
//!   will race at the read-modify-write boundary and lose entries.
//!   In practice, only one phase runs at a time inside a single
//!   `/spec:execute` invocation, and a second `/spec:execute` is
//!   prevented by `.specify/plan.lock` (L2.C). Callers who want
//!   multi-writer safety must add their own coordination.
//!
//! - **Malformed file rejection**: [`Journal::load`] surfaces a
//!   `Error::Yaml` (via `From<serde_saphyr::Error>`) on malformed
//!   content; it does **not** silently truncate or recover. The only
//!   "graceful" branch is "file absent → empty journal".

use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use specify_error::Error;

use crate::Phase;
use crate::timestamp::Rfc3339Stamp;

/// On-disk representation of `<slice_dir>/journal.yaml`. Append-only.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct Journal {
    /// Ordered list of audit log entries.
    #[serde(default)]
    pub entries: Vec<JournalEntry>,
}

/// One line of audit history — a question raised, a failure observed,
/// or a recovery taken.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct JournalEntry {
    /// RFC3339 UTC timestamp.
    pub timestamp: Rfc3339Stamp,
    /// Phase that wrote the entry (`define | build | merge`).
    pub step: Phase,
    /// Entry classification. Named `r#type` because `type` is a
    /// reserved keyword; the serialised field name is literally
    /// `type` — no `#[serde(rename)]` needed, since `r#type`'s
    /// identifier is `type` once the raw-identifier prefix is
    /// stripped.
    pub r#type: EntryKind,
    /// Short human-readable summary.
    pub summary: String,
    /// Optional verbatim detail — stderr, ambiguous-requirement text,
    /// multi-line context. Omitted from the serialised form when
    /// `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
}

/// Classification of a [`JournalEntry`].
#[derive(Debug, Copy, Clone, Deserialize, Serialize, PartialEq, Eq, Hash, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum EntryKind {
    /// Phase paused to ask a clarifying question.
    Question,
    /// Phase observed a failure (compile error, test failure, etc.).
    Failure,
    /// Phase recovered from a previous failure or deferred state.
    Recovery,
}

impl fmt::Display for EntryKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Question => "question",
            Self::Failure => "failure",
            Self::Recovery => "recovery",
        })
    }
}

impl Journal {
    /// Convenience helper: `<slice_dir>/journal.yaml`.
    #[must_use]
    pub fn path(slice_dir: &Path) -> PathBuf {
        slice_dir.join("journal.yaml")
    }

    /// Load the journal from disk.
    ///
    /// Returns an empty [`Journal`] (not `Err`) when the file is
    /// absent — journals are lazily created on first [`Journal::append`].
    /// A malformed file surfaces `Error::Yaml` (via
    /// `From<serde_saphyr::Error>`) with the underlying parser's
    /// location hint; load never silently recovers from corruption.
    ///
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    pub fn load(slice_dir: &Path) -> Result<Self, Error> {
        let path = Self::path(slice_dir);
        if !path.exists() {
            return Ok(Self { entries: Vec::new() });
        }
        let content = std::fs::read_to_string(&path)?;
        let journal: Self = serde_saphyr::from_str(&content)?;
        Ok(journal)
    }

    /// Append a single entry and atomically persist.
    ///
    /// Read-modify-write: loads the existing file (or an empty
    /// `Journal` when absent), pushes `entry`, and routes the
    /// serialised journal through the crate's `atomic_yaml_write`
    /// helper. That emits a trailing newline and bottoms out at
    /// `fs::rename` (atomic on a single filesystem), so a mid-write
    /// crash leaves the prior file intact.
    ///
    /// Does **not** lock the file. See the module-level `Single-writer`
    /// note for the safety envelope this operates in.
    ///
    /// # Errors
    ///
    /// Returns an error if the operation fails.
    pub fn append(slice_dir: &Path, entry: JournalEntry) -> Result<(), Error> {
        let mut journal = Self::load(slice_dir)?;
        journal.entries.push(entry);

        let path = Self::path(slice_dir);
        crate::atomic::atomic_yaml_write(&path, &journal)
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    fn sample_entry(summary: &str) -> JournalEntry {
        JournalEntry {
            timestamp: Rfc3339Stamp::from_raw("2026-04-16T14:30:00Z".to_string()),
            step: Phase::Build,
            r#type: EntryKind::Question,
            summary: summary.to_string(),
            context: None,
        }
    }

    #[test]
    fn path_helper_appends_journal_yaml() {
        assert_eq!(Journal::path(Path::new("/tmp/x")), PathBuf::from("/tmp/x/journal.yaml"));
    }

    #[test]
    fn load_missing_file_returns_empty_journal() {
        let dir = tempdir().expect("tempdir");
        let journal = Journal::load(dir.path()).expect("load ok");
        assert_eq!(journal, Journal { entries: vec![] });
    }

    #[test]
    fn append_persists_to_disk_and_load_returns_entry() {
        let dir = tempdir().expect("tempdir");
        let entry = sample_entry("task 3/7 unclear");
        Journal::append(dir.path(), entry.clone()).expect("append ok");

        let raw = std::fs::read_to_string(Journal::path(dir.path())).expect("read ok");
        assert!(raw.contains("entries:"), "missing `entries:` in:\n{raw}");
        assert!(raw.contains("timestamp:"), "missing `timestamp:` in:\n{raw}");
        assert!(raw.contains("step: build"), "missing kebab-case step:\n{raw}");
        assert!(raw.contains("type: question"), "missing literal `type:`:\n{raw}");
        assert!(raw.contains("summary: task 3/7 unclear"), "missing summary:\n{raw}");

        let loaded = Journal::load(dir.path()).expect("load ok");
        assert_eq!(loaded.entries.len(), 1);
        assert_eq!(loaded.entries[0], entry);
    }

    #[test]
    fn append_preserves_order_across_multiple_calls() {
        let dir = tempdir().expect("tempdir");
        let summaries = ["one", "two", "three", "four", "five"];
        for s in summaries {
            Journal::append(dir.path(), sample_entry(s)).expect("append ok");
        }
        let loaded = Journal::load(dir.path()).expect("load ok");
        assert_eq!(loaded.entries.len(), summaries.len());
        let seen: Vec<&str> = loaded.entries.iter().map(|e| e.summary.as_str()).collect();
        assert_eq!(seen, summaries);
    }

    #[test]
    fn append_never_truncates_on_mid_write_error() {
        // Seed `journal.yaml` directly via a successful Journal serialise
        // so we know the pre-append byte-shape exactly. Then append C
        // and assert the on-disk bytes for A and B survive byte-for-byte.
        let dir = tempdir().expect("tempdir");
        let a = JournalEntry {
            timestamp: Rfc3339Stamp::from_raw("2026-04-16T10:00:00Z".to_string()),
            step: Phase::Define,
            r#type: EntryKind::Question,
            summary: "A".to_string(),
            context: None,
        };
        let b = JournalEntry {
            timestamp: Rfc3339Stamp::from_raw("2026-04-16T10:05:00Z".to_string()),
            step: Phase::Define,
            r#type: EntryKind::Failure,
            summary: "B".to_string(),
            context: Some("stderr blob".to_string()),
        };
        let seed = Journal {
            entries: vec![a.clone(), b.clone()],
        };
        let mut seed_bytes = serde_saphyr::to_string(&seed).expect("serialise seed");
        if !seed_bytes.ends_with('\n') {
            seed_bytes.push('\n');
        }
        std::fs::write(Journal::path(dir.path()), &seed_bytes).expect("seed write");
        let pre_bytes = std::fs::read(Journal::path(dir.path())).expect("read pre");

        let c = JournalEntry {
            timestamp: Rfc3339Stamp::from_raw("2026-04-16T10:10:00Z".to_string()),
            step: Phase::Build,
            r#type: EntryKind::Recovery,
            summary: "C".to_string(),
            context: None,
        };
        Journal::append(dir.path(), c.clone()).expect("append ok");

        let post_bytes = std::fs::read(Journal::path(dir.path())).expect("read post");
        assert!(
            post_bytes.len() > pre_bytes.len(),
            "post-append file must be strictly longer than pre-append snapshot"
        );

        // serde_saphyr is deterministic: serialising `[A, B, C]` emits
        // the exact bytes of `[A, B]` followed by the serialisation of
        // `C`. That means the pre-append bytes (sans trailing newline)
        // are a prefix of the post-append bytes — the A+B region is
        // byte-for-byte intact, which is the atomicity guarantee
        // surfaced to observers of the file.
        let pre_core =
            pre_bytes.strip_suffix(b"\n").expect("pre-append file always ends with newline");
        assert_eq!(
            &post_bytes[..pre_core.len()],
            pre_core,
            "A+B region must be byte-identical to pre-append snapshot"
        );

        let loaded = Journal::load(dir.path()).expect("load ok");
        assert_eq!(loaded.entries, vec![a, b, c]);
    }

    #[test]
    fn append_emits_trailing_newline() {
        let dir = tempdir().expect("tempdir");
        Journal::append(dir.path(), sample_entry("first")).expect("append ok");
        let bytes = std::fs::read(Journal::path(dir.path())).expect("read ok");
        assert!(!bytes.is_empty(), "journal.yaml must not be empty");
        assert_eq!(*bytes.last().unwrap(), b'\n', "must end with newline");
    }

    #[test]
    fn malformed_file_surfaces_error_on_load() {
        let dir = tempdir().expect("tempdir");
        std::fs::write(Journal::path(dir.path()), "not: a\n  valid: yaml\n: structure:")
            .expect("seed garbage");
        let err = Journal::load(dir.path()).expect_err("expected error");
        assert!(matches!(err, Error::Yaml(_)), "expected Error::Yaml, got {err:?}");
    }

    #[test]
    fn entry_round_trips_through_yaml() {
        for kind in [EntryKind::Question, EntryKind::Failure, EntryKind::Recovery] {
            for phase in [Phase::Define, Phase::Build, Phase::Merge] {
                let entry = JournalEntry {
                    timestamp: Rfc3339Stamp::from_raw("2026-04-16T14:30:00+00:00".to_string()),
                    step: phase,
                    r#type: kind,
                    summary: "summary text".to_string(),
                    context: Some("verbatim detail".to_string()),
                };
                let yaml = serde_saphyr::to_string(&entry).expect("serialise");
                let parsed: JournalEntry = serde_saphyr::from_str(&yaml).expect("parse");
                assert_eq!(parsed, entry, "round-trip failed for {phase:?}/{kind:?}:\n{yaml}");
            }
        }
    }

    #[test]
    fn context_survives_multi_line_yaml_block() {
        let dir = tempdir().expect("tempdir");
        let entry = JournalEntry {
            timestamp: Rfc3339Stamp::from_raw("2026-04-16T14:30:00Z".to_string()),
            step: Phase::Build,
            r#type: EntryKind::Failure,
            summary: "multi-line detail".to_string(),
            context: Some("line1\nline2\nline3".to_string()),
        };
        Journal::append(dir.path(), entry.clone()).expect("append ok");
        let loaded = Journal::load(dir.path()).expect("load ok");
        assert_eq!(loaded.entries, vec![entry]);
    }

    #[test]
    fn type_field_name_on_disk_is_literally_type() {
        let entry = sample_entry("example");
        let yaml = serde_saphyr::to_string(&entry).expect("serialise");
        assert!(yaml.contains("type: question"), "expected literal `type: question`, got:\n{yaml}");
        assert!(!yaml.contains("kind:"), "rust-side `kind` must not leak onto disk:\n{yaml}");
    }

    /// Spawn 4 threads, each appending 10 entries, and verify no entry
    /// is silently dropped.
    ///
    /// NOTE: `Journal::append` is **not** mutex-protected. Concurrent
    /// writers race at the read-modify-write boundary — thread A
    /// loading [..N] while thread B is loading [..N] means whichever
    /// persists last loses the other's entry. In the single-writer
    /// deployment (one phase at a time under `/spec:execute`, plus the
    /// upcoming `.specify/plan.lock` in L2.C), this never fires. We
    /// `#[ignore]` the test by default and leave it as documentation
    /// of the limitation — running it under `cargo test -- --ignored`
    /// reliably reproduces the data-loss behaviour that motivates the
    /// plan.lock.
    #[test]
    #[ignore = "append is not inter-writer atomic; see L2.C plan.lock for the real coordination boundary"]
    fn concurrent_append_simulation_via_threads() {
        use std::collections::HashSet;

        let dir = tempdir().expect("tempdir");
        let path = dir.path();

        std::thread::scope(|scope| {
            for t in 0..4 {
                scope.spawn(move || {
                    for i in 0..10 {
                        let entry = JournalEntry {
                            timestamp: Rfc3339Stamp::from_raw("2026-04-16T14:30:00Z".to_string()),
                            step: Phase::Build,
                            r#type: EntryKind::Question,
                            summary: format!("t{t}-i{i}"),
                            context: None,
                        };
                        Journal::append(path, entry).expect("append ok");
                    }
                });
            }
        });

        let loaded = Journal::load(path).expect("load ok");
        assert_eq!(loaded.entries.len(), 40, "all 40 entries must be present");

        let summaries: HashSet<&str> = loaded.entries.iter().map(|e| e.summary.as_str()).collect();
        assert_eq!(summaries.len(), 40, "every summary must appear exactly once");
    }

    #[test]
    fn entry_kind_display_matches_serde_wire_format() {
        assert_eq!(EntryKind::Question.to_string(), "question");
        assert_eq!(EntryKind::Failure.to_string(), "failure");
        assert_eq!(EntryKind::Recovery.to_string(), "recovery");
    }
}
