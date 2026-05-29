//! Integration coverage for the framework link reference/directive checks.

use std::fs;
use std::path::{Path, PathBuf};

use specify_lints::framework::check::links::run_on_root;
use specify_lints::framework::{core_id_for, snippet};

fn fixtures_base() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/links")
}

fn assemble_fixture(case: &str) -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().to_path_buf();
    copy_dir_all(&fixtures_base().join("scaffold"), &root);
    copy_dir_all(&fixtures_base().join(case), &root);
    (temp, root)
}

fn copy_dir_all(from: &Path, to: &Path) {
    if !from.is_dir() {
        return;
    }
    fs::create_dir_all(to).expect("create target dir");
    for entry in fs::read_dir(from).expect("read fixture dir") {
        let entry = entry.expect("dir entry");
        let target = to.join(entry.file_name());
        if entry.file_type().expect("file type").is_dir() {
            copy_dir_all(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).expect("copy fixture file");
        }
    }
}

#[test]
fn reference_links_flag_missing() {
    let (_temp, root) = assemble_fixture("reference_broken");
    let findings: Vec<_> = run_on_root(&root)
        .into_iter()
        .filter(|finding| finding.rule_id.as_deref() == core_id_for("links.broken-reference"))
        .collect();
    assert_eq!(findings.len(), 1);
    assert!(snippet(&findings[0]).contains("references/missing.md"));
}

#[test]
fn reference_links_ignore_fences() {
    let (_temp, root) = assemble_fixture("reference_ignored");
    let findings = run_on_root(&root);
    assert!(findings.is_empty(), "expected no findings, got: {findings:?}");
}

#[test]
fn skill_directives_flag_unknown_skill() {
    let (_temp, root) = assemble_fixture("directive_bad_skill");
    let findings = run_on_root(&root);
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].rule_id.as_deref(), core_id_for("links.unresolved-directive"));
    assert!(snippet(&findings[0]).contains("skill 'demo:missing' not found"));
}

#[test]
fn ignore_moved_fixtures() {
    let (_temp, root) = assemble_fixture("scaffold");
    let fixture_doc =
        root.join("specify-cli/crates/lints/tests/fixtures/links/directive_bad_plugin/docs");
    fs::create_dir_all(&fixture_doc).expect("create nested fixture dir");
    fs::write(fixture_doc.join("guide.md"), "<!-- skill: missing:test -->\n")
        .expect("write nested fixture doc");

    let findings = run_on_root(&root);
    assert!(findings.is_empty(), "expected no findings, got: {findings:?}");
}
