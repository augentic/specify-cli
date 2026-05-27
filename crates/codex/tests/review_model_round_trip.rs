//! Round-trip + schema validation for the RFC-32 `WorkspaceModel`
//! DTOs.
//!
//! Validates two invariants the indexer (S6) relies on:
//!
//! 1. The empty envelope serialises to a wire shape that satisfies
//!    `specify_schema::WORKSPACE_MODEL_JSON_SCHEMA` and round-trips
//!    back to the same Rust value.
//! 2. Populating one record per entity family also serialises into
//!    a schema-valid envelope and round-trips back — catches
//!    per-entity `rename_all` regressions the empty fixture would
//!    miss.

use serde_json::{Map, Value, json};
use specify_codex::review::{
    AdapterAxis, AdapterManifest, CodexRuleFact, File, FileKind, Frontmatter, MarkdownLink,
    MarkdownSection, MarketplaceEntry, ScanProfile, Skill, Symlink, TextMatch, WorkspaceModel,
    WorkspaceModelVersion,
};
use specify_codex::rules::Origin;
use specify_error::ValidationStatus;
use specify_schema::{WORKSPACE_MODEL_JSON_SCHEMA, validate_value};

fn assert_schema_valid(value: &Value) {
    let summaries = validate_value(
        value,
        WORKSPACE_MODEL_JSON_SCHEMA,
        "workspace-model",
        "WorkspaceModel round-trip fixture",
    );
    let failures: Vec<_> =
        summaries.iter().filter(|s| matches!(s.status, ValidationStatus::Fail)).collect();
    assert!(failures.is_empty(), "WorkspaceModel must validate; got {failures:?}");
}

#[test]
fn empty_workspace_model_round_trips_through_schema() {
    let model = WorkspaceModel {
        version: WorkspaceModelVersion,
        project_dir: ".".into(),
        scan_profile: ScanProfile::Consumer,
        artifact_paths: vec![],
        languages: vec![],
        files: vec![],
        frontmatter: vec![],
        markdown_sections: vec![],
        markdown_links: vec![],
        symlinks: vec![],
        skills: vec![],
        adapter_manifests: vec![],
        marketplace_entries: vec![],
        codex_rules: vec![],
        text_matches: vec![],
    };

    let value = serde_json::to_value(&model).expect("serialise empty model");

    assert_eq!(value.get("version"), Some(&Value::from(1)));
    assert_eq!(value.get("project_dir").and_then(Value::as_str), Some("."));
    assert_eq!(value.get("scan_profile").and_then(Value::as_str), Some("consumer"));
    for required_array in [
        "artifact_paths",
        "languages",
        "files",
        "frontmatter",
        "markdown_sections",
        "markdown_links",
        "symlinks",
        "skills",
        "adapter_manifests",
        "marketplace_entries",
        "codex_rules",
        "text_matches",
    ] {
        assert!(
            value.get(required_array).and_then(Value::as_array).is_some_and(Vec::is_empty),
            "envelope must always serialise `{required_array}` as an empty array"
        );
    }

    assert_schema_valid(&value);

    let parsed: WorkspaceModel =
        serde_json::from_value(value).expect("round-trip empty model from JSON");
    assert_eq!(model, parsed);
}

#[test]
fn populated_workspace_model_round_trips_through_schema() {
    let mut frontmatter_fields: Map<String, Value> = Map::new();
    frontmatter_fields.insert("title".into(), json!("Refine"));
    frontmatter_fields.insert("description".into(), json!("Refine a Specify slice"));

    let model = WorkspaceModel {
        version: WorkspaceModelVersion,
        project_dir: ".".into(),
        scan_profile: ScanProfile::Consumer,
        artifact_paths: vec!["src/lib.rs".into()],
        languages: vec!["rust".into()],
        files: vec![File {
            path: "src/lib.rs".into(),
            kind: FileKind::Text,
            language: Some("rust".into()),
            sha256: Some("0".repeat(64)),
        }],
        frontmatter: vec![Frontmatter {
            path: "plugins/spec/skills/refine/SKILL.md".into(),
            schema_id: Some("skill".into()),
            fields: frontmatter_fields,
        }],
        markdown_sections: vec![MarkdownSection {
            path: "README.md".into(),
            level: 2,
            title: "Overview".into(),
            line_start: 5,
            line_end: 12,
            body_line_count: 6,
        }],
        markdown_links: vec![MarkdownLink {
            from_path: "README.md".into(),
            to_raw: "./docs/index.md".into(),
            line: 7,
            resolves: Some(true),
        }],
        symlinks: vec![Symlink {
            path: "adapters/targets/omnia/references/agent-teams.md".into(),
            target: "../../../shared/agent-teams.md".into(),
            broken: false,
        }],
        skills: vec![Skill {
            name: "refine".into(),
            path: "plugins/spec/skills/refine/SKILL.md".into(),
            plugin: "spec".into(),
            frontmatter_ref: "plugins/spec/skills/refine/SKILL.md".into(),
        }],
        adapter_manifests: vec![AdapterManifest {
            axis: AdapterAxis::Targets,
            name: "omnia".into(),
            path: "adapters/targets/omnia/adapter.yaml".into(),
            version: Some("1".into()),
        }],
        marketplace_entries: vec![MarketplaceEntry {
            plugin: "spec".into(),
            path_in_manifest: "/plugins/0".into(),
        }],
        codex_rules: vec![CodexRuleFact {
            rule_id: "UNI-014".into(),
            path: "adapters/shared/codex/universal/hardcoded-configuration.md".into(),
            origin: Origin::Shared,
            frontmatter_ref: "adapters/shared/codex/universal/hardcoded-configuration.md".into(),
        }],
        text_matches: vec![TextMatch {
            path: "src/lib.rs".into(),
            line: 1,
            column: 1,
            pattern_id: "url".into(),
        }],
    };

    let value = serde_json::to_value(&model).expect("serialise populated model");

    // Per-entity rename-all spot checks — these catch regressions
    // where a `rename_all = "kebab-case"` annotation is dropped or
    // miswired on a single entity struct.
    let section =
        value.pointer("/markdown_sections/0").expect("populated markdown_sections has index 0");
    assert!(section.get("line-start").is_some(), "markdown_sections.line-start missing");
    assert!(section.get("line_start").is_none(), "snake_case must not leak from markdownSection");

    let link = value.pointer("/markdown_links/0").expect("populated markdown_links has index 0");
    assert!(link.get("from-path").is_some(), "markdown_links.from-path missing");
    assert!(link.get("from_path").is_none());

    let skill = value.pointer("/skills/0").expect("populated skills has index 0");
    assert!(skill.get("frontmatter-ref").is_some(), "skill.frontmatter-ref missing");
    assert!(skill.get("frontmatter_ref").is_none());

    let entry =
        value.pointer("/marketplace_entries/0").expect("populated marketplace_entries has index 0");
    assert!(
        entry.get("path-in-manifest").is_some(),
        "marketplace_entries.path-in-manifest missing"
    );
    assert!(entry.get("path_in_manifest").is_none());

    let rule = value.pointer("/codex_rules/0").expect("populated codex_rules has index 0");
    assert!(rule.get("rule-id").is_some(), "codex_rules.rule-id missing");
    assert!(rule.get("rule_id").is_none());

    let text = value.pointer("/text_matches/0").expect("populated text_matches has index 0");
    assert!(text.get("pattern-id").is_some(), "text_matches.pattern-id missing");
    assert!(text.get("pattern_id").is_none());

    let fm = value.pointer("/frontmatter/0").expect("populated frontmatter has index 0");
    assert!(fm.get("schema-id").is_some(), "frontmatter.schema-id missing");
    assert!(fm.get("schema_id").is_none());

    assert_schema_valid(&value);

    let parsed: WorkspaceModel =
        serde_json::from_value(value).expect("round-trip populated model from JSON");
    assert_eq!(model, parsed);
}
