//! Shared fixtures, seeds, and re-exports for the `plan_orchestrate`
//! integration suite (REVIEW.md A13).
//!
//! The suite was a single 2,900-line file; it is now split across the
//! sibling `#[path]` submodules (`lifecycle`, `archive`, `authority`,
//! `propose`). Every submodule pulls its shared surface in with
//! `use crate::support::*;`, so the common imports, helpers, and plan
//! seeds live here once.

pub(crate) use std::fs;
pub(crate) use std::path::{Path, PathBuf};
pub(crate) use std::process::Command as ProcessCommand;

pub(crate) use serde_json::Value;
pub(crate) use specify_workflow::change::Plan;
pub(crate) use tempfile::{TempDir, tempdir};

pub(crate) use crate::common::{
    Project, assert_golden_at, copy_dir, init_hub, omnia_schema_dir, parse_stderr, parse_stdout,
    repo_root, specrun,
};

pub(crate) fn plan_fixtures() -> PathBuf {
    repo_root().join("tests/fixtures/plan")
}

pub(crate) fn assert_golden(name: &str, actual: Value) {
    assert_golden_at(&plan_fixtures(), name, actual);
}

// -- test seeds --------------------------------------------------------

pub(crate) const CLEAN_PLAN: &str = "\
name: demo
slices:
  - name: a
    project: default
    status: pending
  - name: b
    project: default
    status: pending
    depends-on: [a]
";

pub(crate) const DUPLICATE_NAME_PLAN: &str = "\
name: demo
slices:
  - name: foo
    project: default
    status: pending
  - name: foo
    project: default
    status: pending
";

pub(crate) const A_DONE_B_PENDING: &str = "\
name: demo
slices:
  - name: a
    project: default
    status: done
  - name: b
    project: default
    status: pending
";

pub(crate) const A_IN_PROGRESS: &str = "\
name: demo
slices:
  - name: a
    project: default
    status: in-progress
";

pub(crate) const ALL_DONE: &str = "\
name: demo
slices:
  - name: a
    project: default
    status: done
  - name: b
    project: default
    status: done
";

/// All entries done — `next` reports `drained` post-2.0 (the
/// previous "stuck" semantics relied on the now-removed `failed`
/// state). Kept under the historical name for fixture continuity;
/// the test asserts the new `drained` reason.
pub(crate) const STUCK_PLAN: &str = "\
name: demo
slices:
  - name: a
    project: default
    status: done
  - name: b
    project: default
    status: done
    depends-on: [a]
";
