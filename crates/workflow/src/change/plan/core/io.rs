//! Disk I/O for `plan.yaml`: atomic load and save.
//!
//! Filesystem moves for archival live in [`super::archive`].

use std::path::{Path, PathBuf};

use specify_error::Error;
use specify_model::atomic::yaml_write;

use super::model::Plan;
use crate::config::{AtomicYaml, Layout};
use crate::schema::validate_plan_yaml;

impl AtomicYaml for Plan {
    fn layout_path(layout: Layout<'_>) -> PathBuf {
        layout.plan_path()
    }

    /// Trait-side loader: `Ok(None)` when the file is absent, mirroring
    /// the contract documented on [`AtomicYaml::load_state`]. Disambiguated
    /// from the inherent [`Plan::load`] (which returns
    /// `Error::ArtifactNotFound` on absence) so the trait helper can
    /// branch on `None` without inspecting the error variant.
    fn load_state(layout: Layout<'_>) -> Result<Option<Self>, Error> {
        let path = Self::layout_path(layout);
        if !path.exists() {
            return Ok(None);
        }
        Self::load(&path).map(Some)
    }
}

impl Plan {
    /// Load `plan.yaml` (at the repo root) from disk.
    ///
    /// Errors mirror [`crate::slice::SliceMetadata::load`]:
    ///   - missing file -> `Error::ArtifactNotFound`
    ///   - schema failure -> `Error::Validation`
    ///   - YAML/type deserialization failure -> `Error::YamlDe`
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
        validate_plan_yaml(&content)?;
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
    /// Returns `Error::Io` on any I/O failure and `Error::YamlSer` if
    /// serialization fails.
    pub fn save(&self, path: &Path) -> Result<(), Error> {
        yaml_write(path, self)
    }
}

#[cfg(test)]
mod tests {
    use specify_error::ValidationStatus;
    use tempfile::tempdir;

    use super::super::model::Status;
    use super::super::{PLAN_EXAMPLE_YAML, change, change_with_deps, plan_with_changes};
    use super::*;

    #[test]
    fn save_load_round_trips() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("plan.yaml");
        let original: Plan = serde_saphyr::from_str(PLAN_EXAMPLE_YAML).expect("parse plan fixture");
        original.save(&path).expect("save ok");
        let loaded = Plan::load(&path).expect("load ok");
        assert_eq!(loaded, original, "full plan should round-trip through save -> load");
    }

    #[test]
    fn save_emits_trailing_newline() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("plan.yaml");
        let mut plan = plan_with_changes(vec![]);
        plan.name = "init".into();
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

        let mut plan = plan_with_changes(vec![change("only-entry", Status::Pending)]);
        plan.name = "fresh".into();
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
    fn load_rejects_rogue_top_level_field() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("plan.yaml");
        std::fs::write(&path, "name: foo\nrogue: true\nslices: []\n").expect("write rogue plan");

        let err = Plan::load(&path).expect_err("rogue top-level field should fail schema");
        let Error::Validation { results } = err else {
            panic!("expected Error::Validation, got {err:?}");
        };
        assert_eq!(results.len(), 1, "expected one schema result, got {results:?}");
        assert_eq!(results[0].status, ValidationStatus::Fail);
        assert_eq!(results[0].rule_id, "plan-schema");
        let detail = results[0].detail.as_deref().expect("schema failure detail");
        assert!(detail.contains("/rogue"), "expected detail to mention `/rogue`, got {detail}");
    }

    #[test]
    fn save_writes_kebab_case() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("plan.yaml");
        let mut plan =
            plan_with_changes(vec![change_with_deps("entry-one", Status::InProgress, &["foo"])]);
        plan.name = "demo".into();
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

        let mut first = plan_with_changes(vec![]);
        first.name = "first".into();
        first.save(&path).expect("save first ok");

        let mut second = plan_with_changes(vec![change("new-entry", Status::Pending)]);
        second.name = "second".into();
        second.save(&path).expect("save second ok");

        let loaded = Plan::load(&path).expect("load ok");
        assert_eq!(loaded, second, "after a successful save, only the new content is observable");
        assert_ne!(loaded, first, "the previous plan should no longer be on disk");

        let bytes = std::fs::read(&path).expect("read bytes");
        assert!(!bytes.is_empty(), "saved file should not be empty after overwrite");
        assert_eq!(*bytes.last().unwrap(), b'\n', "overwritten file should still end with newline");
    }
}
