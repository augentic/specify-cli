//! `SliceMetadata` / `Outcome` serde round-trips, plus the top-level
//! `--help` axis-verb surface.

use crate::support::*;

#[test]
fn metadata_without_outcome_still_parses() {
    use specify_workflow::slice::SliceMetadata;
    // Hand-craft a `.metadata.yaml` that predates the `outcome` field
    // and assert that SliceMetadata::load accepts it and leaves
    // `outcome` as None.
    let tmp = tempfile::tempdir().expect("tempdir");
    let slice_dir = tmp.path();
    let yaml = r#"target: omnia
status: refining
created-at: "2024-08-01T10:00:00Z"
"#;
    fs::write(slice_dir.join(".metadata.yaml"), yaml).expect("write metadata");
    let meta = SliceMetadata::load(slice_dir).expect("legacy metadata parses");
    assert!(
        meta.outcome.is_none(),
        "pre-existing metadata without an outcome field must load as None"
    );
}

#[test]
fn phase_outcome_round_trips_serde() {
    use specify_workflow::slice::Outcome;
    // Construction via struct literal would require crossing the
    // `#[non_exhaustive]` boundary on `Outcome`; round-trip through
    // YAML instead so the wire shape is what's exercised.
    for kind in ["success", "failure", "deferred"] {
        for phase in ["shape", "build", "merge"] {
            let yaml = format!(
                "phase: {phase}\noutcome: {kind}\nat: \"2024-08-01T10:00:00Z\"\nsummary: some summary\n"
            );
            let parsed: Outcome = serde_saphyr::from_str(&yaml).expect("parse");
            let reserialised = serde_saphyr::to_string(&parsed).expect("serialize");
            let reparsed: Outcome = serde_saphyr::from_str(&reserialised).expect("reparse");
            assert_eq!(parsed, reparsed, "round-trip failed for yaml:\n{yaml}");
        }
    }
}

// ---- Top-level help surfaces source/target axis verbs ----

#[test]
fn help_lists_axis_verbs() {
    let assert = specrun().arg("--help").assert().success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8 stdout");
    assert!(stdout.contains("slice"), "Top-level --help must still list `slice`, got:\n{stdout}");
    assert!(
        stdout.lines().any(|line| line.trim_start().starts_with("source ")),
        "Top-level --help must list the `source` axis verb, got:\n{stdout}"
    );
    assert!(
        stdout.lines().any(|line| line.trim_start().starts_with("target ")),
        "Top-level --help must list the `target` axis verb, got:\n{stdout}"
    );
    assert!(
        !stdout.lines().any(|line| line.trim_start().starts_with("change ")),
        "Top-level --help must NOT list the retired `change` verb, got:\n{stdout}"
    );
    assert!(
        !stdout.lines().any(|line| line.trim_start().starts_with("adapter ")),
        "Top-level --help must NOT list the retired `adapter` verb, got:\n{stdout}"
    );
}
