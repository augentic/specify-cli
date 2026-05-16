//! Disk I/O for `plan.yaml`: atomic load and save.
//!
//! Filesystem moves for archival live in [`super::archive`].

use std::path::{Path, PathBuf};

use specify_error::Error;

use super::model::Plan;
use crate::config::{AtomicYaml, Layout};
use crate::slice::atomic::yaml_write;

impl AtomicYaml for Plan {
    fn path(layout: Layout<'_>) -> PathBuf {
        layout.plan_path()
    }

    /// `plan.yaml` is created by `specify change draft` (alongside
    /// `change.md`), never synthesised implicitly. Mutation handlers
    /// (`add`, `amend`,
    /// `transition`) should drive `with_state` with
    /// [`crate::config::InitPolicy::RequireExisting`] to surface
    /// absence as a typed [`Error::ArtifactNotFound`].
    fn default_for_load() -> Option<Self> {
        None
    }

    /// Trait-side loader: `Ok(None)` when the file is absent, mirroring
    /// the contract documented on [`AtomicYaml::load`]. Disambiguated
    /// from the inherent [`Plan::load`] (which returns
    /// `Error::ArtifactNotFound` on absence) so the trait helper can
    /// branch on `None` without inspecting the error variant. The
    /// explicit `Plan::` prefix selects the inherent associated
    /// function; `Self::load` would resolve to this trait method and
    /// recurse.
    #[expect(
        clippy::use_self,
        reason = "explicit type prefix disambiguates the inherent `Plan::load` from this trait method of the same name"
    )]
    fn load(layout: Layout<'_>) -> Result<Option<Self>, Error> {
        let path = Self::path(layout);
        if !path.exists() {
            return Ok(None);
        }
        Plan::load(&path).map(Some)
    }
}

#[expect(
    clippy::same_name_method,
    reason = "inherent `Plan::load` is intentionally shadowed by the `AtomicYaml::load` trait impl in `config/atomic.rs`; the trait impl delegates to this fn"
)]
impl Plan {
    /// Load `plan.yaml` (at the repo root) from disk.
    ///
    /// Errors mirror [`crate::slice::SliceMetadata::load`]:
    ///   - missing file -> `Error::ArtifactNotFound`
    ///   - malformed YAML -> `Error::Yaml`
    ///   - other I/O failure -> `Error::Io`
    ///
    /// Tolerant of files with or without a trailing newline —
    /// `serde_saphyr::from_str` accepts both.
    ///
    /// # Errors
    ///
    /// See variants enumerated above.
    pub fn load(path: &Path) -> Result<Self, Error> {
        if !path.exists() {
            return Err(Error::ArtifactNotFound {
                kind: "plan.yaml",
                path: path.to_path_buf(),
            });
        }
        let content = std::fs::read_to_string(path)?;
        let plan: Self = serde_saphyr::from_str(&content)?;
        Ok(plan)
    }

    /// Serialize and write the plan to `path`, overwriting if present.
    ///
    /// Atomic: a partial file is never observed by readers. Write goes via
    /// a temp file in the same directory followed by `fs::rename`. Because
    /// POSIX `rename(2)` (and Windows `MoveFileEx` with `MOVEFILE_REPLACE_EXISTING`)
    /// are atomic at the filesystem level, any concurrent reader of `path`
    /// sees either the previous complete contents or the new complete
    /// contents — never a half-written or empty file.
    ///
    /// Always emits a trailing newline so the on-disk form matches the
    /// convention used elsewhere in the project and so POSIX text-file
    /// tools (`wc -l`, `sed`, `grep`) behave predictably.
    ///
    /// # Errors
    ///
    /// Returns `Error::Io` on any I/O failure and `Error::Yaml` if
    /// serialization fails.
    pub fn save(&self, path: &Path) -> Result<(), Error> {
        yaml_write(path, self)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use tempfile::tempdir;

    use super::super::model::{Entry, Status};
    use super::super::test_support::RFC_EXAMPLE_YAML;
    use super::*;

    #[test]
    fn save_load_round_trips() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("plan.yaml");
        let original: Plan = serde_saphyr::from_str(RFC_EXAMPLE_YAML).expect("parse rfc fixture");
        original.save(&path).expect("save ok");
        let loaded = Plan::load(&path).expect("load ok");
        assert_eq!(loaded, original, "full plan should round-trip through save -> load");
    }

    #[test]
    fn save_emits_trailing_newline() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("plan.yaml");
        let plan = Plan {
            name: "init".to_string(),
            sources: BTreeMap::new(),
            entries: vec![],
        };
        plan.save(&path).expect("save ok");

        let bytes = std::fs::read(&path).expect("read ok");
        assert!(!bytes.is_empty(), "saved file should not be empty");
        assert_eq!(*bytes.last().unwrap(), b'\n', "saved file should end with a newline");

        let content = std::str::from_utf8(&bytes).expect("utf8");
        assert!(
            content.contains("name: init"),
            "file should contain `name: init`, got:\n{content}"
        );
    }

    #[test]
    fn save_overwrites_atomically() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("plan.yaml");
        std::fs::write(&path, "garbage that should be overwritten").expect("write garbage");

        let plan = Plan {
            name: "fresh".to_string(),
            sources: BTreeMap::new(),
            entries: vec![Entry {
                name: "only-entry".to_string(),
                project: Some("default".into()),
                capability: None,
                status: Status::Pending,
                depends_on: vec![],
                sources: vec![],
                context: vec![],
                description: None,
                status_reason: None,
            }],
        };
        plan.save(&path).expect("save ok");

        let loaded = Plan::load(&path).expect("load ok");
        assert_eq!(loaded, plan, "loaded plan should equal saved plan");

        let raw = std::fs::read_to_string(&path).expect("read ok");
        assert!(
            !raw.contains("garbage"),
            "pre-existing garbage content should be gone, got:\n{raw}"
        );
    }

    #[test]
    fn load_missing_returns_not_found() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("does-not-exist.yaml");
        let err = Plan::load(&path).expect_err("expected error on missing file");
        match err {
            Error::ArtifactNotFound { kind, path: p } => {
                assert_eq!(kind, "plan.yaml");
                assert_eq!(p, path);
            }
            other => panic!("expected Error::ArtifactNotFound, got {other:?}"),
        }
    }

    #[test]
    fn load_no_trailing_newline() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("plan.yaml");
        std::fs::write(&path, "name: foo\nslices: []").expect("write without trailing newline");
        let plan = Plan::load(&path).expect("load ok");
        assert_eq!(plan.name, "foo");
        assert!(plan.entries.is_empty());
    }

    #[test]
    fn save_writes_kebab_case() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("plan.yaml");
        let plan = Plan {
            name: "demo".to_string(),
            sources: BTreeMap::new(),
            entries: vec![Entry {
                name: "entry-one".to_string(),
                project: Some("default".into()),
                capability: None,
                status: Status::InProgress,
                depends_on: vec!["foo".to_string()],
                sources: vec![],
                context: vec![],
                description: None,
                status_reason: None,
            }],
        };
        plan.save(&path).expect("save ok");

        let content = std::fs::read_to_string(&path).expect("read ok");
        assert!(
            content.contains("depends-on:"),
            "expected kebab-case `depends-on:`, got:\n{content}"
        );
        assert!(
            content.contains("status: in-progress"),
            "expected kebab-case enum value `in-progress`, got:\n{content}"
        );
        assert!(
            !content.contains("depends_on"),
            "snake_case `depends_on` leaked onto disk, got:\n{content}"
        );
        assert!(
            !content.contains("in_progress"),
            "snake_case `in_progress` leaked onto disk, got:\n{content}"
        );
    }

    #[test]
    fn save_no_intermediate_state() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("plan.yaml");

        let first = Plan {
            name: "first".to_string(),
            sources: BTreeMap::new(),
            entries: vec![],
        };
        first.save(&path).expect("save first ok");

        let second = Plan {
            name: "second".to_string(),
            sources: BTreeMap::new(),
            entries: vec![Entry {
                name: "new-entry".to_string(),
                project: Some("default".into()),
                capability: None,
                status: Status::Pending,
                depends_on: vec![],
                sources: vec![],
                context: vec![],
                description: None,
                status_reason: None,
            }],
        };
        second.save(&path).expect("save second ok");

        let loaded = Plan::load(&path).expect("load ok");
        assert_eq!(loaded, second, "after a successful save, only the new content is observable");
        assert_ne!(loaded, first, "the previous plan should no longer be on disk");

        let bytes = std::fs::read(&path).expect("read bytes");
        assert!(!bytes.is_empty(), "saved file should not be empty after overwrite");
        assert_eq!(*bytes.last().unwrap(), b'\n', "overwritten file should still end with newline");
    }
}
