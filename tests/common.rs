//! Helpers shared across the binary's integration tests.
//!
//! Each test file `mod common;` to pull these in (cargo's "include
//! shared module" idiom for `tests/`). Some test files use only a
//! subset, so the items are tagged `#[allow(dead_code)]` to keep
//! lints quiet.

use std::fs;
use std::path::Path;

/// Recursively copy `src` into `dst`, creating directories as needed.
///
/// # Panics
///
/// Panics if a fixture directory cannot be read or copied into the test
/// workspace.
#[allow(dead_code)]
pub fn copy_dir(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).expect("create_dir_all dst");
    for entry in fs::read_dir(src).expect("read_dir src") {
        let entry = entry.expect("dir entry");
        let kind = entry.file_type().expect("file_type");
        let target = dst.join(entry.file_name());
        if kind.is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).expect("copy");
        }
    }
}
