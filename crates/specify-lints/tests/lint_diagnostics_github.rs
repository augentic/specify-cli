//! `Format::Github` formatter — workflow-annotation lines plus
//! escape-rule coverage.

mod common;

use specify_lints::lint::diagnostics::{Format, render};

use crate::common::make_fixture;

#[test]
fn one_line_per_finding() {
    let fixture = make_fixture();
    let rendered = render(Format::Github, &fixture).expect("github render succeeds");
    let lines: Vec<&str> = rendered.lines().collect();
    assert_eq!(lines.len(), fixture.findings.len(), "one annotation per finding");

    let critical = lines[0];
    assert!(critical.starts_with("::error "), "critical must map to ::error; got {critical}");
    assert!(
        critical.contains("file=crates/invoice_export/src/config.rs"),
        "expected file= argument; got {critical}"
    );
    assert!(critical.contains(",line=18,"), "expected line argument; got {critical}");
    assert!(critical.contains(",col=5,"), "expected col argument; got {critical}");
    assert!(critical.contains("[UNI-014]"), "rule id should appear in message body");
    assert!(critical.contains("Impact"));
    assert!(critical.contains("Remediation"));

    let important = lines[1];
    assert!(important.starts_with("::error "), "important must also map to ::error");
    assert!(
        !important.contains(",col="),
        "no col= argument when finding has no column; got {important}"
    );
    assert!(important.contains("file=tests/fixtures/blob.bin"));
    // Title contains a comma; the argument-list escape must replace
    // it with %2C so the workflow-command parser keeps the title in
    // one argument.
    assert!(
        important.contains("title=Bundle digest%2C with comma%2C exceeds policy"),
        "comma in title must be escaped as %2C; got {important}"
    );

    let optional = lines[2];
    assert!(optional.starts_with("::notice "), "optional must map to ::notice; got {optional}");
    assert!(!optional.contains("file="), "no file= argument when finding has no location");
    assert!(optional.contains("title=Optional housekeeping note"));
}
