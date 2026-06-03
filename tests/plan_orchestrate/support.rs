//! Shared fixtures, seeds, and re-exports for the `plan_orchestrate`
//! integration suite (REVIEW.md A13).
//!
//! The suite is split across themed submodules grouped by `plan`
//! command family (`validate`, `next`, `mutate`, `source_binding`,
//! `transition`, `create`, `archive`, `propose`, `authority`). Every
//! submodule pulls its shared surface in with `use crate::support::*;`,
//! so the common imports, helpers, and plan seeds live here once.

pub use std::fs;
pub use std::path::{Path, PathBuf};
pub use std::process::Command as ProcessCommand;

pub use serde_json::Value;
pub use specify_workflow::change::Plan;
pub use tempfile::{TempDir, tempdir};

pub use crate::common::{
    Project, assert_golden_at, copy_dir, init_workspace, omnia_schema_dir, parse_stderr,
    parse_stdout, repo_root, specify_cmd,
};

pub fn plan_fixtures() -> PathBuf {
    repo_root().join("tests/fixtures/plan")
}

pub fn assert_golden(name: &str, actual: Value) {
    assert_golden_at(&plan_fixtures(), name, actual);
}

// -- test seeds --------------------------------------------------------

pub const CLEAN_PLAN: &str = "\
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

pub const DUPLICATE_NAME_PLAN: &str = "\
name: demo
slices:
  - name: foo
    project: default
    status: pending
  - name: foo
    project: default
    status: pending
";

pub const A_DONE_B_PENDING: &str = "\
name: demo
slices:
  - name: a
    project: default
    status: done
  - name: b
    project: default
    status: pending
";

pub const A_IN_PROGRESS: &str = "\
name: demo
slices:
  - name: a
    project: default
    status: in-progress
";

/// One pending entry. Shared by the `mutate` (amend-on-missing) and
/// `transition` submodules.
pub const SINGLE_PENDING: &str = "\
name: demo
slices:
  - name: foo
    project: default
    status: pending
";

pub const ALL_DONE: &str = "\
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
pub const STUCK_PLAN: &str = "\
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
