//! `Format::Compact` formatter — tab-separated one-line-per-finding.

mod common;

use specify_codex::review::diagnostics::{Format, render};

use crate::common::make_fixture;

#[test]
fn compact_formatter_emits_one_tsv_line_per_finding() {
    let fixture = make_fixture();
    let rendered = render(Format::Compact, &fixture).expect("compact render succeeds");
    assert!(rendered.ends_with('\n'));
    let lines: Vec<&str> = rendered.lines().collect();
    assert_eq!(lines.len(), fixture.findings.len(), "one line per finding");

    let mut iter = lines.iter().copied();
    assert_eq!(
        iter.next().expect("line 0"),
        "critical\tUNI-014\tcrates/invoice_export/src/config.rs:18:5\tLiteral deployment URL in generated handler"
    );
    assert_eq!(
        iter.next().expect("line 1"),
        "important\t-\ttests/fixtures/blob.bin:42:-\tBundle digest, with comma, exceeds policy"
    );
    assert_eq!(
        iter.next().expect("line 2"),
        "optional\tORG-001\t-:-:-\tOptional housekeeping note"
    );
}
