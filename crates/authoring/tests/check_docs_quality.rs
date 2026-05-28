use std::fs;
use std::path::Path;

use specify_authoring::Context;
use specify_authoring::check::{
    HistoryCitation, MissingDiagramAsset, TextPipelineDiagram,
};
use specify_authoring::finding::Check;

fn scaffold_framework_root(root: &Path) {
    fs::create_dir_all(root.join("plugins/spec")).expect("plugins dir");
    fs::create_dir_all(root.join("adapters/sources")).expect("adapters dir");
}

fn ctx_at(root: &Path) -> Context {
    Context::from_framework_root(root).expect("framework root")
}

#[test]
fn specify_history_citation_flags_user_facing_docs() {
    let dir = tempfile::tempdir().expect("tempdir");
    scaffold_framework_root(dir.path());
    fs::create_dir_all(dir.path().join("docs/tutorials")).expect("docs dir");
    fs::write(
        dir.path().join("docs/tutorials/guide.md"),
        format!("See {}-5 for the background.\n", "R".to_owned() + "FC"),
    )
    .expect("write md");

    let findings = HistoryCitation.run(&ctx_at(dir.path()));
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].rule_id, "docs.specify-history-citation-in-docs");
    assert!(findings[0].message.contains("docs/tutorials/guide.md:1"));
}

#[test]
fn missing_diagram_asset_flags_broken_svg_ref() {
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
    assert_eq!(findings[0].rule_id, "docs.missing-diagram-asset");
    assert!(findings[0].message.contains("missing.svg"));
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
    assert_eq!(findings[0].rule_id, "docs.text-pipeline-diagram");
    assert!(findings[0].message.contains("docs/explanation/overview.md"));
}
