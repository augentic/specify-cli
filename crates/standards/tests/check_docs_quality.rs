//! Integration coverage for the framework docs-quality checks.

use std::fs;
use std::path::Path;

use specify_standards::framework::check::{
    Check, HistoryCitation, MissingDiagramAsset, TextPipelineDiagram,
};
use specify_standards::framework::{Context, core_id_for, snippet};

fn scaffold_framework_root(root: &Path) {
    fs::create_dir_all(root.join("plugins/spec")).expect("plugins dir");
    fs::create_dir_all(root.join("adapters/sources")).expect("adapters dir");
}

fn ctx_at(root: &Path) -> Context {
    Context::from_framework_root(root).expect("framework root")
}

#[test]
fn history_citation_flags_docs() {
    let dir = tempfile::tempdir().expect("tempdir");
    scaffold_framework_root(dir.path());
    fs::create_dir_all(dir.path().join("docs/tutorials")).expect("docs dir");
    fs::write(
        dir.path().join("docs/tutorials/guide.md"),
        format!("See {}{}-5 for the background.\n", "R", "FC"),
    )
    .expect("write md");

    let findings = HistoryCitation.run(&ctx_at(dir.path()));
    assert_eq!(findings.len(), 1);
    assert_eq!(
        findings[0].rule_id.as_deref(),
        core_id_for("docs.specify-history-citation-in-docs")
    );
    assert!(snippet(&findings[0]).contains("docs/tutorials/guide.md:1"));
}

#[test]
fn missing_diagram_flags_broken_svg() {
    let dir = tempfile::tempdir().expect("tempdir");
    scaffold_framework_root(dir.path());
    fs::create_dir_all(dir.path().join("docs/reference")).expect("docs dir");
    fs::write(
        dir.path().join("docs/reference/page.md"),
        "![flow](../assets/diagrams/missing.svg)\n",
    )
    .expect("write md");

    let findings = MissingDiagramAsset.run(&ctx_at(dir.path()));
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].rule_id.as_deref(), core_id_for("docs.missing-diagram-asset"));
    assert!(snippet(&findings[0]).contains("missing.svg"));
}

#[test]
fn text_pipeline_diagram_flags_arrow_flow() {
    let dir = tempfile::tempdir().expect("tempdir");
    scaffold_framework_root(dir.path());
    fs::create_dir_all(dir.path().join("docs/explanation")).expect("docs dir");
    fs::write(
        dir.path().join("docs/explanation/overview.md"),
        "Pipeline:\n\n```text\nA -> B\n```\n",
    )
    .expect("write md");

    let findings = TextPipelineDiagram.run(&ctx_at(dir.path()));
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].rule_id.as_deref(), core_id_for("docs.text-pipeline-diagram"));
    assert!(snippet(&findings[0]).contains("docs/explanation/overview.md"));
}
