//! Integration coverage for the framework plugin symlink/marketplace checks.

use std::fs;
use std::io::Write;
use std::path::Path;

use specify_standards::framework::check::{BrokenSymlinkCheck, Check, MarketplaceDriftCheck};
use specify_standards::framework::{Context, core_id_for, snippet};

fn fixture_context(root: &Path) -> Context {
    Context::from_framework_root(root).expect("fixture framework root")
}

fn write_framework_scaffold(root: &Path) {
    fs::create_dir_all(root.join("adapters")).expect("adapters dir");
    fs::create_dir_all(root.join("plugins")).expect("plugins dir");
}

fn write_valid_marketplace(root: &Path, plugins: &[(&str, &str)]) {
    let entries: Vec<String> = plugins
        .iter()
        .map(|(name, source)| {
            format!(
                r#"    {{
      "name": "{name}",
      "source": "{source}",
      "description": "Test plugin {name}."
    }}"#
            )
        })
        .collect();
    let body = format!(
        r#"{{
  "name": "test-marketplace",
  "owner": {{ "name": "test", "email": "test@example.com" }},
  "metadata": {{
    "description": "Test marketplace fixture.",
    "version": "0.0.1",
    "pluginRoot": "plugins"
  }},
  "plugins": [
{}
  ]
}}"#,
        entries.join(",\n")
    );
    let manifest_dir = root.join(".cursor-plugin");
    fs::create_dir_all(&manifest_dir).expect("marketplace dir");
    fs::write(manifest_dir.join("marketplace.json"), body).expect("marketplace json");
}

fn write_plugin_surface(root: &Path, source: &str) {
    let plugin_dir = root.join("plugins").join(source);
    fs::create_dir_all(plugin_dir.join("skills")).expect("skills dir");
    fs::create_dir_all(plugin_dir.join(".cursor-plugin")).expect("plugin manifest dir");
    fs::write(
        plugin_dir.join(".cursor-plugin").join("plugin.json"),
        r#"{"name":"test","version":"0.0.1"}"#,
    )
    .expect("plugin json");
}

#[test]
fn broken_symlink_reports_unresolved() {
    let temp = tempfile::tempdir().expect("temp dir");
    write_framework_scaffold(temp.path());
    fs::create_dir_all(temp.path().join("plugins")).expect("plugins dir");
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        symlink("../missing-target", temp.path().join("plugins/broken-link"))
            .expect("broken symlink");
    };
    #[cfg(not(unix))]
    {
        return;
    }

    let ctx = fixture_context(temp.path());
    let findings = BrokenSymlinkCheck.run(&ctx);
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].rule_id.as_deref(), core_id_for("plugins.broken-symlink"));
    assert!(snippet(&findings[0]).contains("broken-link"));
}

#[test]
fn marketplace_drift_reports_undeclared() {
    let temp = tempfile::tempdir().expect("temp dir");
    write_framework_scaffold(temp.path());
    write_valid_marketplace(temp.path(), &[("declared", "declared")]);
    write_plugin_surface(temp.path(), "declared");
    write_plugin_surface(temp.path(), "orphan");

    let ctx = fixture_context(temp.path());
    let findings = MarketplaceDriftCheck.run(&ctx);
    assert!(
        findings.iter().any(|finding| {
            finding.rule_id.as_deref() == core_id_for("plugins.marketplace-drift")
                && snippet(finding).contains("orphan")
                && snippet(finding).contains("not in marketplace.json")
        }),
        "expected undeclared plugin finding, got {findings:?}"
    );
}

#[test]
fn marketplace_drift_reports_schema() {
    let temp = tempfile::tempdir().expect("temp dir");
    write_framework_scaffold(temp.path());
    let manifest_dir = temp.path().join(".cursor-plugin");
    fs::create_dir_all(&manifest_dir).expect("marketplace dir");
    let mut file = fs::File::create(manifest_dir.join("marketplace.json")).expect("manifest");
    write!(
        file,
        r#"{{
  "name": "bad",
  "owner": {{ "name": "test", "email": "not-an-email" }},
  "metadata": {{
    "description": "Bad marketplace.",
    "version": "0.0.1",
    "pluginRoot": "plugins"
  }},
  "plugins": []
}}"#
    )
    .expect("write invalid marketplace");

    let ctx = fixture_context(temp.path());
    let findings = MarketplaceDriftCheck.run(&ctx);
    assert!(
        findings.iter().any(|finding| {
            finding.rule_id.as_deref() == core_id_for("plugins.marketplace-drift")
                && snippet(finding).contains("schema violation")
        }),
        "expected schema violation finding, got {findings:?}"
    );
}
