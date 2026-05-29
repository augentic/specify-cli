//! Integration coverage for the framework skill body discipline checks.

use std::fs;
use std::path::{Path, PathBuf};

use specify_lints::framework::check::{
    Check, EnvelopeJsonInBody, InvalidCriticalPath, VariableCoverage,
};
use specify_lints::framework::{Context, core_id_for, snippet};

fn fixture_root(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/skill_body").join(name)
}

fn scaffold_framework_root(root: &Path) {
    fs::create_dir_all(root.join("plugins/demo/skills/test")).expect("skill dir");
    fs::create_dir_all(root.join("adapters")).expect("adapters dir");
}

fn write_skill(root: &Path, body: &str) {
    let content = format!(
        "---\nname: test-skill\ndescription: Fixture skill for body discipline checks in authoring tests.\nargument-hint: <arg>\n---\n\n{body}\n"
    );
    fs::write(root.join("plugins/demo/skills/test/SKILL.md"), content).expect("write skill");
}

fn context_for_fixture(name: &str) -> Context {
    let root = fixture_root(name);
    scaffold_framework_root(&root);
    Context::from_framework_root(root).expect("framework root resolves")
}

fn repeated_lines(prefix: &str, count: usize) -> String {
    (0..count).map(|i| format!("{prefix} {i}")).collect::<Vec<_>>().join("\n")
}

#[test]
fn invalid_critical_path_wrong_count() {
    let ctx = context_for_fixture("invalid-critical-path");
    let mut body = String::from("## Critical Path\n\n");
    for i in 1..=4 {
        body.push_str(&format!("{i}. Step {i}\n"));
    }
    body.push('\n');
    body.push_str(&repeated_lines("padding", 150));
    write_skill(&fixture_root("invalid-critical-path"), &body);

    let findings = InvalidCriticalPath.run(&ctx);
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].rule_id.as_deref(), core_id_for("skill.invalid-critical-path"));
    assert!(snippet(&findings[0]).contains("found 4"));
}

#[test]
fn envelope_json_flags_shape() {
    let ctx = context_for_fixture("envelope-json");
    let body = r##"## Output

```json
{
  "envelope-version": "1",
  "ok": true,
  "data": {}
}
```
"##;
    write_skill(&fixture_root("envelope-json"), body);

    let findings = EnvelopeJsonInBody.run(&ctx);
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].rule_id.as_deref(), core_id_for("skill.envelope-json-in-body"));
    assert!(snippet(&findings[0]).contains("Envelope JSON in skill body"));
}

#[test]
fn variable_coverage_flags_undefined_use() {
    let ctx = context_for_fixture("undefined-variable");
    let body = r#"## Arguments

```text
$SLICE=<name>
```

## Steps

Validate $PROJECT for $SLICE before continuing.
"#;
    write_skill(&fixture_root("undefined-variable"), body);

    let findings = VariableCoverage.run(&ctx);
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].rule_id.as_deref(), core_id_for("skill.variable-coverage"));
    assert!(snippet(&findings[0]).contains("Undefined variable"));
    assert!(snippet(&findings[0]).contains("$PROJECT"));
}
