//! `Format::Pretty` formatter — header, per-finding block, summary
//! footer.
//!
//! The test sets `NO_COLOR=1` so the golden stays ANSI-free across
//! environments. Regenerate via:
//!
//! ```text
//! REGENERATE_GOLDENS=1 cargo test --test review_diagnostics_pretty
//! ```

#![expect(
    unsafe_code,
    reason = "env::set_var / env::remove_var are unsafe under Rust 2024; this binary has a single test and no concurrent thread reads NO_COLOR."
)]

mod common;

use std::path::PathBuf;
use std::{env, fs};

use specify_lints::lint::diagnostics::{Format, render};

use crate::common::make_fixture;

fn goldens_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join("goldens")
}

#[track_caller]
fn assert_golden(actual: &str, name: &str) {
    let golden_path = goldens_dir().join(name);
    if env::var_os("REGENERATE_GOLDENS").is_some() {
        fs::create_dir_all(golden_path.parent().expect("golden parent")).expect("mkdir golden");
        fs::write(&golden_path, actual).expect("write golden");
        return;
    }
    let expected = fs::read_to_string(&golden_path).unwrap_or_else(|err| {
        panic!(
            "golden {} missing ({err}); regenerate via \
             REGENERATE_GOLDENS=1 cargo test --test review_diagnostics_pretty",
            golden_path.display()
        )
    });
    assert_eq!(actual, expected, "pretty golden drift; see golden at {}", golden_path.display());
}

/// One test per binary — `NO_COLOR` is process-wide and parallel test
/// execution within a binary would race if the colour-on and
/// colour-off branches lived in separate `#[test]` functions.
#[test]
fn matches_golden_honours_no_color() {
    let fixture = make_fixture();

    // SAFETY: `std::env::set_var` is `unsafe` (env mutation racing
    // with other threads is UB). Within a single test, nextest gives
    // the test a fresh process and no other thread reads `NO_COLOR`
    // before `render` returns.
    let () = unsafe { env::set_var("NO_COLOR", "1") };
    let plain = render(Format::Pretty, &fixture).expect("pretty render with NO_COLOR succeeds");

    assert!(
        plain.starts_with("Specify review — 3 finding(s)\n"),
        "expected header line; got: {plain}"
    );
    assert!(plain.contains("[CRITICAL]"));
    assert!(plain.contains("[IMPORTANT]"));
    assert!(plain.contains("[OPTIONAL]"));
    assert!(plain.contains("Literal deployment URL in generated handler"));
    assert!(plain.contains("Bundle digest, with comma, exceeds policy"));
    assert!(plain.contains("Optional housekeeping note"));
    assert!(plain.contains("Summary: 1 critical, 1 important, 0 suggestion, 1 optional"));
    assert!(!plain.contains("\x1b["), "NO_COLOR must suppress ANSI escapes");

    assert_golden(&plain, "review_diagnostics_pretty.txt");

    // SAFETY: same single-test sequencing argument as above.
    let () = unsafe { env::remove_var("NO_COLOR") };
    let coloured =
        render(Format::Pretty, &fixture).expect("pretty render without NO_COLOR succeeds");
    assert!(coloured.contains("\x1b[31m"), "critical tag should carry the red ANSI escape");
    assert!(coloured.contains("\x1b[0m"), "ANSI escape must be terminated");
}
