//! Parity tests — `tests/fixtures/parity/` holds byte-for-byte
//! outputs captured from the archived Python reference implementation
//! (now retired). The Rust port must match them exactly.

use specify_error::Error;
use specify_merge::{merge, validate_baseline};
use specify_capability::ValidationResult;

macro_rules! fixture {
    ($case:literal, $file:literal) => {
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/fixtures/parity/",
            $case,
            "/",
            $file
        ))
    };
}

fn assert_merge_success(case: &str, baseline: Option<&str>, delta: &str, expected: &str) {
    let result = merge(baseline, delta).unwrap_or_else(|e| {
        panic!("{case}: merge returned Err: {e:?}");
    });
    assert!(
        result.output == expected,
        "{case}: merged output mismatch.\n--- expected ---\n{expected}\n--- actual ---\n{}",
        result.output
    );
}

#[test]
fn case_01_single_req_is_byte_for_byte_identical() {
    assert_merge_success(
        "case-01-single-req",
        Some(fixture!("case-01-single-req", "baseline.md")),
        fixture!("case-01-single-req", "delta.md"),
        fixture!("case-01-single-req", "expected-merged.md"),
    );
    assert!(fixture!("case-01-single-req", "expected-merge-errors.txt").trim().is_empty());
}

#[test]
fn case_02_multi_req_is_byte_for_byte_identical() {
    assert_merge_success(
        "case-02-multi-req",
        Some(fixture!("case-02-multi-req", "baseline.md")),
        fixture!("case-02-multi-req", "delta.md"),
        fixture!("case-02-multi-req", "expected-merged.md"),
    );
}

#[test]
fn case_03_new_baseline_is_byte_for_byte_identical() {
    assert_merge_success(
        "case-03-new-baseline",
        None,
        fixture!("case-03-new-baseline", "delta.md"),
        fixture!("case-03-new-baseline", "expected-merged.md"),
    );
}

#[test]
fn case_04_modified_is_byte_for_byte_identical() {
    assert_merge_success(
        "case-04-modified",
        Some(fixture!("case-04-modified", "baseline.md")),
        fixture!("case-04-modified", "delta.md"),
        fixture!("case-04-modified", "expected-merged.md"),
    );
}

#[test]
fn case_05_removed_is_byte_for_byte_identical() {
    assert_merge_success(
        "case-05-removed",
        Some(fixture!("case-05-removed", "baseline.md")),
        fixture!("case-05-removed", "delta.md"),
        fixture!("case-05-removed", "expected-merged.md"),
    );
}

#[test]
fn case_06_renamed_is_byte_for_byte_identical() {
    assert_merge_success(
        "case-06-renamed",
        Some(fixture!("case-06-renamed", "baseline.md")),
        fixture!("case-06-renamed", "delta.md"),
        fixture!("case-06-renamed", "expected-merged.md"),
    );
}

#[test]
fn case_07_all_sections_is_byte_for_byte_identical() {
    assert_merge_success(
        "case-07-all-sections",
        Some(fixture!("case-07-all-sections", "baseline.md")),
        fixture!("case-07-all-sections", "delta.md"),
        fixture!("case-07-all-sections", "expected-merged.md"),
    );
}

#[test]
fn merge_failure_surfaces_consolidated_error_messages() {
    // No fixture on disk for this case — every repo fixture is expected
    // to succeed, so we hand-craft a small failing pair here.
    let baseline = "### Requirement: A\n\nID: REQ-001\n\n#### Scenario: ok\n\n- ok\n\n### Requirement: B\n\nID: REQ-002\n\n#### Scenario: ok\n\n- ok\n";
    let delta = "## MODIFIED Requirements\n\n### Requirement: Missing\n\nID: REQ-999\n\n#### Scenario: none\n\n- none\n";
    let err = merge(Some(baseline), delta).expect_err("merge should fail");
    match err {
        Error::Merge(msg) => {
            assert!(
                msg.contains("MODIFIED: ID REQ-999 not found in baseline"),
                "missing expected MODIFIED error: {msg}"
            );
        }
        other => panic!("expected Error::Merge, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// validate_baseline parity
// ---------------------------------------------------------------------------

fn fails(results: &[ValidationResult]) -> Vec<&str> {
    results
        .iter()
        .filter_map(|r| match r {
            ValidationResult::Fail { detail, .. } => Some(detail.as_str()),
            _ => None,
        })
        .collect()
}

#[test]
fn case_08_validation_ok_has_no_failures() {
    let baseline = fixture!("case-08-validation-ok", "baseline.md");
    let expected = fixture!("case-08-validation-ok", "expected-validation.txt");
    let results = validate_baseline(baseline, None);
    assert!(fails(&results).is_empty(), "expected no fails; got {:?}", fails(&results));
    assert!(expected.trim().is_empty());
}

#[test]
fn case_09_validation_fails_produces_expected_failure_set() {
    let baseline = fixture!("case-09-validation-fails", "baseline.md");
    let expected = fixture!("case-09-validation-fails", "expected-validation.txt");
    let results = validate_baseline(baseline, None);
    let actual_details = fails(&results);

    let expected_details: Vec<&str> =
        expected.lines().filter_map(|line| line.strip_prefix("FAIL: ")).collect();

    for needle in &expected_details {
        assert!(
            actual_details.iter().any(|actual| actual.contains(needle)),
            "missing expected FAIL line {needle:?} in {actual_details:?}"
        );
    }
    assert_eq!(
        actual_details.len(),
        expected_details.len(),
        "failure count drift: expected {expected_details:?}, got {actual_details:?}"
    );
}

#[test]
fn case_10_design_refs_preserves_python_regex_quirk() {
    // The expected file is empty: Python's `^REQ-[0-9]{3}$` (no
    // MULTILINE) never matches inside the multi-line design body, even
    // though `design.md` mentions REQ-999 / REQ-042 that are not in the
    // baseline. Change G's `cross.design-references-valid` will finally
    // catch those; for now, parity requires zero fails.
    let baseline = fixture!("case-10-design-refs", "baseline.md");
    let design = fixture!("case-10-design-refs", "design.md");
    let expected = fixture!("case-10-design-refs", "expected-validation.txt");
    let results = validate_baseline(baseline, Some(design));
    assert!(fails(&results).is_empty(), "got unexpected fails: {:?}", fails(&results));
    assert!(expected.trim().is_empty());
}
