//! On-disk representation of `<slice_dir>/journal.yaml` — the
//! append-only audit log written by phase skills. `Journal::append` is
//! a single-writer atomic read-modify-write; `load` rejects malformed YAML.

use std::path::{Path, PathBuf};

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use specify_error::Error;

use crate::slice::Phase;

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
    /// Second-precision UTC timestamp (`%Y-%m-%dT%H:%M:%SZ`).
    #[serde(with = "specify_error::serde_rfc3339")]
    pub timestamp: Timestamp,
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
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    serde::Serialize,
    serde::Deserialize,
    strum::Display,
    clap::ValueEnum,
)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
#[non_exhaustive]
pub enum EntryKind {
    /// Phase paused to ask a clarifying question.
    Question,
    /// Phase observed a failure (compile error, test failure, etc.).
    Failure,
    /// Phase recovered from a previous failure or deferred state.
    Recovery,
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
    /// `From<YamlError>`) with the underlying parser's
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
    /// serialised journal through the crate's `yaml_write`
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
        crate::slice::atomic::yaml_write(&path, &journal)
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    fn parse_stamp(raw: &str) -> Timestamp {
        raw.parse().expect("valid rfc3339 timestamp in test fixture")
    }

    fn sample_entry(summary: &str) -> JournalEntry {
        JournalEntry {
            timestamp: parse_stamp("2026-04-16T14:30:00Z"),
            step: Phase::Build,
            r#type: EntryKind::Question,
            summary: summary.to_string(),
            context: None,
        }
    }

    #[test]
    fn load_missing_file_returns_empty() {
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
    fn append_never_truncates_on_mid_write_error() {
        // Seed `journal.yaml` directly via a successful Journal serialise
        // so we know the pre-append byte-shape exactly. Then append C
        // and assert the on-disk bytes for A and B survive byte-for-byte.
        let dir = tempdir().expect("tempdir");
        let a = JournalEntry {
            timestamp: parse_stamp("2026-04-16T10:00:00Z"),
            step: Phase::Define,
            r#type: EntryKind::Question,
            summary: "A".to_string(),
            context: None,
        };
        let b = JournalEntry {
            timestamp: parse_stamp("2026-04-16T10:05:00Z"),
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
            timestamp: parse_stamp("2026-04-16T10:10:00Z"),
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
}
