use super::*;
use crate::framework::builder::{core_id_for, snippet};

#[test]
fn relative_path_strips_framework_root() {
    let temp = tempfile::tempdir().expect("tempdir");
    scaffold_framework(temp.path());
    let ctx = Context::from_framework_root(temp.path()).expect("framework root resolves");
    let path = ctx.sources_dir().join("intent").join(ADAPTER_FILENAME);
    assert_eq!(relative_path(&ctx, &path), "adapters/sources/intent/adapter.yaml");
}

#[test]
fn missing_manifest_on_empty_dir() {
    let temp = tempfile::tempdir().expect("tempdir");
    scaffold_framework(temp.path());
    let adapter_dir = temp.path().join("adapters/sources/broken");
    fs::create_dir_all(&adapter_dir).expect("adapter dir");
    let ctx = Context::from_framework_root(temp.path()).expect("context");
    let findings = check_missing_manifests(&ctx, &ctx.sources_dir());
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].rule_id.as_deref(), core_id_for(RULE_MISSING_MANIFEST));
    assert!(snippet(&findings[0]).contains("adapters/sources/broken"));
}

#[test]
fn execution_agent_emits_suggestion() {
    use specify_diagnostics::Severity;

    let temp = tempfile::tempdir().expect("tempdir");
    scaffold_framework(temp.path());
    let adapter_dir = temp.path().join("adapters/sources/documentation");
    fs::create_dir_all(&adapter_dir).expect("adapter dir");
    fs::write(
            adapter_dir.join(ADAPTER_FILENAME),
            "name: documentation\nversion: 1\naxis: source\nexecution: agent\nbriefs:\n  survey: briefs/survey.md\n  extract: briefs/extract.md\ndescription: Docs source.\n",
        )
        .expect("manifest");
    let ctx = Context::from_framework_root(temp.path()).expect("context");

    let findings = check_execution_agent(&ctx, &ctx.sources_dir());
    assert_eq!(findings.len(), 1, "execution: agent must surface one finding");
    assert_eq!(findings[0].rule_id.as_deref(), core_id_for(RULE_EXECUTION_AGENT));
    assert_eq!(findings[0].severity, Severity::Suggestion, "must not block CI");
    assert!(snippet(&findings[0]).contains("adapters/sources/documentation"));
}

#[test]
fn execution_tool_emits_nothing() {
    let temp = tempfile::tempdir().expect("tempdir");
    scaffold_framework(temp.path());
    let adapter_dir = temp.path().join("adapters/targets/widget");
    fs::create_dir_all(&adapter_dir).expect("adapter dir");
    fs::write(
            adapter_dir.join(ADAPTER_FILENAME),
            "name: widget\nversion: 1\naxis: target\nexecution: tool\nbriefs:\n  shape: briefs/shape.md\n  build: briefs/build.md\n  merge: briefs/merge.md\ndescription: Tool target.\n",
        )
        .expect("manifest");
    let ctx = Context::from_framework_root(temp.path()).expect("context");

    assert!(
        check_execution_agent(&ctx, &ctx.targets_dir()).is_empty(),
        "execution: tool must not surface the agent suggestion"
    );
}

fn scaffold_framework(root: &Path) {
    fs::create_dir_all(root.join("plugins")).expect("plugins");
    fs::create_dir_all(root.join("adapters/sources")).expect("sources");
    fs::create_dir_all(root.join("adapters/targets")).expect("targets");
}
