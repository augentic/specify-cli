//! Integration tests for `specify_domain::adapter`.
//!
//! The "workspace" fixture we test against is the `specify` repo itself:
//! `schemas/omnia/` and `schemas/omnia/briefs/*.md` are real, hand-edited
//! files that every skill already relies on. By pointing
//! `PipelineView::load` at the repo root via `CARGO_MANIFEST_DIR`, we
//! parity-test the crate against those real inputs without checking in a
//! duplicated fixture tree.

use std::path::{Path, PathBuf};

use specify_domain::adapter::{
    Adapter, AdapterSource, Brief, CacheMeta, CodexProvenance, CodexRule, CodexSeverity,
    DEFAULT_CODEX_ADAPTER, Phase, Pipeline, PipelineEntry, PipelineView, ResolvedCodex,
};
use specify_error::{Error, ValidationStatus, ValidationSummary};
use tempfile::TempDir;

/// Absolute path to the repo root (the Cargo workspace root).
fn repo_root() -> PathBuf {
    // CARGO_MANIFEST_DIR = <repo>/crates/domain
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .and_then(|p| p.parent())
        .map(Path::to_path_buf)
        .expect("CARGO_MANIFEST_DIR should have two ancestors (crates/, repo root)")
}

fn omnia_adapter_path() -> PathBuf {
    repo_root().join("schemas").join("omnia").join("adapter.yaml")
}

fn codex_fixture_path(name: &str) -> PathBuf {
    repo_root().join("tests").join("fixtures").join(name)
}

// ---------- Adapter parsing ----------

#[test]
fn parses_omnia_adapter_yaml_fields_and_entries() {
    let raw = std::fs::read_to_string(omnia_adapter_path()).expect("omnia adapter on disk");
    let schema: Adapter = serde_saphyr::from_str(&raw).expect("omnia adapter is valid YAML");

    assert_eq!(schema.name, "omnia");
    assert_eq!(schema.version, 1);
    assert_eq!(schema.description, "Omnia Rust WASM workflow");

    assert_eq!(schema.pipeline.define.len(), 4);
    assert_eq!(schema.pipeline.build.len(), 1);
    assert_eq!(schema.pipeline.merge.len(), 1);

    let define_ids: Vec<&str> = schema.pipeline.define.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(define_ids, vec!["proposal", "specs", "design", "tasks"]);

    assert_eq!(schema.pipeline.build[0].id, "build");
    assert_eq!(schema.pipeline.build[0].brief, "briefs/build.md");
    assert_eq!(schema.pipeline.merge[0].id, "merge");
    assert_eq!(schema.pipeline.merge[0].brief, "briefs/merge.md");

    for entry in schema
        .pipeline
        .define
        .iter()
        .chain(schema.pipeline.build.iter())
        .chain(schema.pipeline.merge.iter())
    {
        assert_eq!(
            entry.brief,
            format!("briefs/{}.md", entry.id),
            "unexpected brief path for id {}",
            entry.id
        );
    }
}

#[test]
fn validate_structure_valid_for_omnia() {
    let raw = std::fs::read_to_string(omnia_adapter_path()).unwrap();
    let schema: Adapter = serde_saphyr::from_str(&raw).unwrap();
    let results = schema.validate_structure();
    assert!(
        results.iter().all(|r| r.status == ValidationStatus::Pass),
        "expected all passes, got: {results:?}"
    );
}

#[test]
fn validate_structure_fails_when_define_phase_is_empty() {
    let schema = Adapter {
        name: "broken".into(),
        version: 1,
        description: "empty define phase".into(),
        pipeline: Pipeline {
            plan: vec![],
            define: vec![],
            build: vec![PipelineEntry {
                id: "build".into(),
                brief: "briefs/build.md".into(),
            }],
            merge: vec![PipelineEntry {
                id: "merge".into(),
                brief: "briefs/merge.md".into(),
            }],
        },
    };

    let results = schema.validate_structure();
    assert!(
        results.iter().any(|r| r.status == ValidationStatus::Fail),
        "expected at least one failure, got: {results:?}"
    );
}

#[test]
fn yaml_parse_error_surface_for_missing_required_field() {
    // `description` missing -> serde error is propagated as an
    // `Error::YamlDe` when surfaced through `Adapter::resolve`, but
    // here we just exercise the parser directly and assert the Display
    // message.
    let yaml = "name: broken\nversion: 1\npipeline:\n  define: []\n  build: []\n  merge: []\n";
    let err = serde_saphyr::from_str::<Adapter>(yaml).expect_err("missing description");
    let message = err.to_string();
    assert!(
        message.contains("description"),
        "expected parse error to mention missing field, got: {message}"
    );
}

// ---------- pipeline.plan (Layer 2 authoring) ----------

#[test]
fn plan_entries_merge_overrides_by_id_and_appends_new_entries() {
    let parent_yaml = r"
name: parent
version: 1
description: parent
pipeline:
  plan:
    - { id: discovery, brief: briefs/plan/discovery.md }
    - { id: propose,   brief: briefs/plan/propose.md }
  define:
    - { id: proposal, brief: briefs/proposal.md }
  build:
    - { id: build, brief: briefs/build.md }
  merge:
    - { id: merge, brief: briefs/merge.md }
";
    let child_yaml = r"
name: child
version: 1
description: child
pipeline:
  plan:
    - { id: propose, brief: briefs/plan/propose-v2.md }
    - { id: record,  brief: briefs/plan/record.md }
  define:
    - { id: proposal, brief: briefs/proposal.md }
  build:
    - { id: build, brief: briefs/build.md }
  merge:
    - { id: merge, brief: briefs/merge.md }
";
    let parent: Adapter = serde_saphyr::from_str(parent_yaml).unwrap();
    let child: Adapter = serde_saphyr::from_str(child_yaml).unwrap();
    let merged = Adapter::merge(parent, child);

    let ids: Vec<&str> = merged.plan_entries().iter().map(|e| e.id.as_str()).collect();
    assert_eq!(ids, vec!["discovery", "propose", "record"]);
    let propose = merged.plan_entries().iter().find(|e| e.id == "propose").unwrap();
    assert_eq!(propose.brief, "briefs/plan/propose-v2.md");
}

// ---------- Manifest composition ----------

#[test]
fn merge_overrides_by_id_and_appends_new_entries() {
    let parent_yaml = r"
name: parent
version: 1
description: parent adapter
pipeline:
  define:
    - { id: proposal, brief: briefs/proposal.md }
    - { id: specs,    brief: briefs/specs.md }
  build:
    - { id: build, brief: briefs/build.md }
  merge:
    - { id: merge, brief: briefs/merge.md }
";
    let child_yaml = r"
name: child
version: 2
description: child adapter
pipeline:
  define:
    - { id: specs,   brief: briefs/specs-v2.md }
    - { id: review,  brief: briefs/review.md }
  build:
    - { id: build, brief: briefs/build.md }
  merge:
    - { id: merge, brief: briefs/merge.md }
";

    let parent: Adapter = serde_saphyr::from_str(parent_yaml).unwrap();
    let child: Adapter = serde_saphyr::from_str(child_yaml).unwrap();
    let merged = Adapter::merge(parent, child);

    assert_eq!(merged.name, "child");
    assert_eq!(merged.version, 2);

    let ids: Vec<&str> = merged.pipeline.define.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(ids, vec!["proposal", "specs", "review"]);

    let specs = &merged.pipeline.define.iter().find(|e| e.id == "specs").unwrap().brief;
    assert_eq!(specs, "briefs/specs-v2.md", "child override took effect");
}

// ---------- entries / entry ----------

#[test]
fn entries_iterates_in_phase_order_and_entry_lookup_works() {
    let raw = std::fs::read_to_string(omnia_adapter_path()).unwrap();
    let schema: Adapter = serde_saphyr::from_str(&raw).unwrap();

    let total =
        schema.pipeline.define.len() + schema.pipeline.build.len() + schema.pipeline.merge.len();
    assert_eq!(schema.entries().count(), total);

    let phases: Vec<Phase> = schema.entries().map(|(p, _)| p).collect();
    let expected_phases: Vec<Phase> = vec![
        Phase::Define,
        Phase::Define,
        Phase::Define,
        Phase::Define,
        Phase::Build,
        Phase::Merge,
    ];
    assert_eq!(phases, expected_phases);

    let (phase, entry) = schema.entry("proposal").expect("proposal is a define entry");
    assert_eq!(phase, Phase::Define);
    assert_eq!(entry.id, "proposal");

    assert!(schema.entry("no-such-id").is_none());
}

// ---------- Brief frontmatter ----------

#[test]
fn parses_every_omnia_brief_and_frontmatter_ids_match_pipeline_ids() {
    let raw = std::fs::read_to_string(omnia_adapter_path()).unwrap();
    let schema: Adapter = serde_saphyr::from_str(&raw).unwrap();
    let root = repo_root().join("schemas").join("omnia");

    for (_phase, entry) in schema.entries() {
        let brief_path = root.join(&entry.brief);
        let brief = Brief::load(&brief_path).expect("brief loads cleanly");
        assert_eq!(
            brief.frontmatter.id,
            entry.id,
            "brief at {} has id `{}` but pipeline entry id is `{}`",
            brief_path.display(),
            brief.frontmatter.id,
            entry.id
        );
        assert!(!brief.frontmatter.description.is_empty());
    }
}

#[test]
fn parses_brief_with_crlf_line_endings() {
    let path = PathBuf::from("virtual.md");
    let contents = "---\r\nid: proposal\r\ndescription: demo\r\n---\r\nbody\r\n";
    let brief = Brief::parse(&path, contents).expect("CRLF frontmatter should parse");
    assert_eq!(brief.frontmatter.id, "proposal");
    assert!(brief.body.starts_with("body"));
}

#[test]
fn brief_parse_rejects_missing_frontmatter() {
    let path = PathBuf::from("no-frontmatter.md");
    let contents = "# plain markdown\nno frontmatter at all\n";
    let err = Brief::parse(&path, contents).expect_err("missing frontmatter");
    assert!(matches!(err, Error::Diag { .. }), "got: {err:?}");
}

#[test]
fn brief_parse_rejects_missing_closing_delimiter() {
    let path = PathBuf::from("unclosed.md");
    let contents = "---\nid: proposal\ndescription: demo\n";
    let err = Brief::parse(&path, contents).expect_err("missing closing ---");
    match err {
        Error::Diag { detail: msg, .. } => assert!(msg.contains("closing `---`"), "msg: {msg}"),
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn brief_parse_rejects_invalid_yaml_frontmatter() {
    let path = PathBuf::from("bad-yaml.md");
    let contents = "---\nid: [unclosed\n---\nbody\n";
    let err = Brief::parse(&path, contents).expect_err("malformed YAML");
    assert!(matches!(err, Error::Diag { .. }), "got: {err:?}");
}

// ---------- Codex rule frontmatter ----------

fn read_codex_fixture(name: &str) -> String {
    std::fs::read_to_string(codex_fixture_path(name)).unwrap_or_else(|err| {
        panic!("failed to read codex fixture `{name}`: {err}");
    })
}

fn codex_fail_detail(results: &[ValidationSummary], rule_id: &str) -> String {
    results
        .iter()
        .find(|r| r.status == ValidationStatus::Fail && r.rule_id == rule_id)
        .and_then(|r| r.detail.clone())
        .unwrap_or_else(|| panic!("expected codex failure `{rule_id}`, got: {results:?}"))
}

fn write_codex_adapter(project_dir: &Path, name: &str, version: u32) -> PathBuf {
    let root = project_dir.join("schemas").join(name);
    std::fs::create_dir_all(&root).expect("create adapter root");
    std::fs::write(
        root.join("adapter.yaml"),
        format!(
            "\
name: {name}
version: {version}
description: {name} test adapter
pipeline:
  define: []
  build: []
  merge: []
"
        ),
    )
    .expect("write adapter manifest");
    root
}

fn write_codex_rule(source_root: &Path, relative_path: &str, id: &str) -> PathBuf {
    let path = source_root.join("codex").join(relative_path);
    let parent = path.parent().expect("codex rule path should have parent");
    std::fs::create_dir_all(parent).expect("create codex rule directory");
    std::fs::write(
        &path,
        format!(
            "\
---
id: {id}
title: Test Rule {id}
severity: suggestion
trigger: Test trigger for {id}.
---

## Rule

Body for {id}.
"
        ),
    )
    .expect("write codex rule");
    path
}

fn resolved_ids(codex: &ResolvedCodex) -> Vec<&str> {
    codex.rules.iter().map(|resolved| resolved.rule.normalized_id.as_str()).collect()
}

#[test]
fn codex_parse_accepts_minimal_rule() {
    let path = codex_fixture_path("codex-valid-minimal.md");
    let contents = read_codex_fixture("codex-valid-minimal.md");
    let rule = CodexRule::parse(&path, &contents).expect("minimal codex rule parses");

    assert_eq!(rule.path, path);
    assert_eq!(rule.frontmatter.id, "UNI-002");
    assert_eq!(rule.normalized_id, "UNI-002");
    assert_eq!(rule.frontmatter.severity, CodexSeverity::Critical);
    assert!(rule.body.contains("## Rule"));

    let results = CodexRule::validate_str(&rule.path, &contents);
    assert!(
        results.iter().all(|result| result.status == ValidationStatus::Pass),
        "valid minimal fixture should pass all codex checks, got: {results:?}"
    );
}

#[test]
fn codex_resolver_orders_sources_and_ignores_specify_codex() {
    let tmp = TempDir::new().expect("tempdir");
    let project_dir = tmp.path();
    let default_root = write_codex_adapter(project_dir, "default", 1);
    let project_root = write_codex_adapter(project_dir, "project", 2);

    write_codex_rule(&default_root, "z-last.md", "UNI-002");
    write_codex_rule(&default_root, "nested/a-first.md", "UNI-001");
    write_codex_rule(&project_root, "a.md", "OMNIA-001");
    write_codex_rule(project_dir, "repo.md", "ORG-001");
    write_codex_rule(&project_dir.join(".specify"), "ignored.md", "SEC-001");

    let codex =
        ResolvedCodex::resolve(project_dir, Some("project"), false).expect("codex resolves");

    assert_eq!(resolved_ids(&codex), vec!["UNI-001", "UNI-002", "OMNIA-001", "ORG-001"]);
    assert_eq!(
        codex.rules[0].provenance,
        CodexProvenance::Adapter {
            name: "default".to_string(),
            version: 1,
        }
    );
    assert_eq!(
        codex.rules[2].provenance,
        CodexProvenance::Adapter {
            name: "project".to_string(),
            version: 2,
        }
    );
    assert_eq!(codex.rules[3].provenance, CodexProvenance::Repo);
}

#[test]
fn codex_resolver_rejects_duplicate_ids_with_validation_error() {
    let tmp = TempDir::new().expect("tempdir");
    let project_dir = tmp.path();
    let default_root = write_codex_adapter(project_dir, "default", 1);
    let project_root = write_codex_adapter(project_dir, "project", 1);

    write_codex_rule(&default_root, "default.md", "UNI-001");
    write_codex_rule(&project_root, "project.md", "OMNIA-001");
    write_codex_rule(project_dir, "repo.md", "UNI-001");

    let err = ResolvedCodex::resolve(project_dir, Some("project"), false)
        .expect_err("duplicate codex id should fail validation");

    let Error::Validation { results } = err else {
        panic!("expected validation error");
    };
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].rule_id, "codex.rule-id-unique");
    let detail = results[0].detail.as_deref().expect("duplicate detail");
    assert!(detail.contains("codex-rule-id-duplicate"), "detail: {detail}");
    assert!(detail.contains("UNI-001"), "detail: {detail}");
}

#[test]
fn codex_resolver_hub_skips_project_adapter_and_keeps_repo_overlay() {
    let tmp = TempDir::new().expect("tempdir");
    let project_dir = tmp.path();
    let default_root = write_codex_adapter(project_dir, "default", 1);

    write_codex_rule(&default_root, "default.md", "UNI-001");
    write_codex_rule(project_dir, "repo.md", "ORG-001");

    let codex = ResolvedCodex::resolve(project_dir, None, true).expect("hub codex resolves");

    assert_eq!(resolved_ids(&codex), vec!["UNI-001", "ORG-001"]);
    assert_eq!(codex.rules[1].provenance, CodexProvenance::Repo);
}

#[test]
fn codex_resolver_names_missing_default_adapter() {
    let tmp = TempDir::new().expect("tempdir");
    let err = ResolvedCodex::resolve(tmp.path(), None, true)
        .expect_err("missing default adapter should fail");

    let Error::Diag { code, detail } = err else {
        panic!("expected schema resolution error");
    };
    assert_eq!(code, "codex-default-adapter-unavailable", "detail: {detail}");
    assert!(
        detail.contains(DEFAULT_CODEX_ADAPTER),
        "detail should name default codex adapter: {detail}"
    );
}

#[test]
fn codex_validate_rejects_malformed_yaml() {
    let path = PathBuf::from("codex/bad-yaml.md");
    let contents = "---\nid: [unclosed\n---\n\n## Rule\n\nbody\n";
    let results = CodexRule::validate_str(&path, contents);
    let detail = codex_fail_detail(&results, "codex.frontmatter-parseable");

    assert!(detail.contains("codex-rule-frontmatter-malformed"), "detail: {detail}");
    let err = CodexRule::parse(&path, contents).expect_err("malformed YAML should fail");
    let Error::Validation { results } = err else {
        panic!("expected validation error");
    };
    assert_eq!(results.len(), 1);
}

#[test]
fn codex_validate_rejects_missing_frontmatter() {
    let path = PathBuf::from("codex/no-frontmatter.md");
    let contents = "# Just markdown\n\n## Rule\n\nbody\n";
    let results = CodexRule::validate_str(&path, contents);
    let detail = codex_fail_detail(&results, "codex.frontmatter-delimited");

    assert!(detail.contains("codex-rule-frontmatter-missing"), "detail: {detail}");
    let err = CodexRule::parse(&path, contents).expect_err("missing frontmatter should fail");
    let Error::Validation { results } = err else {
        panic!("expected validation error");
    };
    assert_eq!(results.len(), 1);
}

#[test]
fn codex_validate_rejects_missing_rule_heading() {
    let path = PathBuf::from("codex/no-rule-heading.md");
    let contents = "\
---
id: UNI-002
title: Unvalidated Input
severity: critical
trigger: External input enters the system.
---

## Guidance

body
";
    let results = CodexRule::validate_str(&path, contents);
    let detail = codex_fail_detail(&results, "codex.body-has-rule-heading");

    assert!(detail.contains("codex-rule-heading-missing"), "detail: {detail}");
    let err = CodexRule::parse(&path, contents).expect_err("missing rule heading should fail");
    let Error::Validation { results } = err else {
        panic!("expected validation error");
    };
    assert_eq!(results.len(), 1);
}

#[test]
fn codex_validate_rejects_missing_required_frontmatter_fields() {
    let path = codex_fixture_path("codex-invalid-missing-title.md");
    let contents = read_codex_fixture("codex-invalid-missing-title.md");
    let results = CodexRule::validate_str(&path, &contents);
    let detail = codex_fail_detail(&results, "codex.frontmatter-valid");

    assert!(detail.contains("title"), "detail should mention missing title: {detail}");
}

#[test]
fn codex_validate_rejects_malformed_rule_ids() {
    let path = codex_fixture_path("codex-invalid-malformed-id.md");
    let contents = read_codex_fixture("codex-invalid-malformed-id.md");
    let results = CodexRule::validate_str(&path, &contents);
    let detail = codex_fail_detail(&results, "codex.frontmatter-valid");

    assert!(detail.contains("/id"), "detail should point at id: {detail}");
}

#[test]
fn codex_validate_rejects_legacy_severity_labels() {
    for severity in ["Warning", "Info"] {
        let path = PathBuf::from(format!("codex/{severity}.md"));
        let contents = format!(
            "\
---
id: UNI-002
title: Legacy Severity
severity: {severity}
trigger: External input enters the system.
---

## Rule

body
"
        );
        let results = CodexRule::validate_str(&path, &contents);
        let detail = codex_fail_detail(&results, "codex.frontmatter-valid");

        assert!(
            detail.contains("/severity"),
            "detail for `{severity}` should point at severity: {detail}"
        );
    }
}

// ---------- PipelineView ----------

#[test]
fn pipeline_view_loads_omnia_schema_from_workspace() {
    let root = repo_root();
    let view = PipelineView::load("omnia", &root).expect("omnia view loads");
    assert_eq!(view.briefs.len(), 6);
    assert!(matches!(view.adapter.source, AdapterSource::Local(_)));

    assert!(view.brief("proposal").is_some());
    assert!(view.brief("build").is_some());
    assert!(view.brief("nope").is_none());

    assert_eq!(view.phase(Phase::Define).count(), 4);
    assert_eq!(view.phase(Phase::Build).count(), 1);
    assert_eq!(view.phase(Phase::Merge).count(), 1);

    let build = view.brief("build").unwrap();
    assert_eq!(build.frontmatter.tracks.as_deref(), Some("tasks"));
    assert_eq!(build.frontmatter.needs, vec!["specs", "design", "tasks"]);
}

/// Scaffold a minimal local adapter at `<project>/schemas/<name>/`
/// with the given `adapter.yaml` and brief contents. Each brief content
/// map entry is `(filename, contents)` written under `schemas/<name>/`.
fn scaffold_schema_project(name: &str, schema_yaml: &str, briefs: &[(&str, &str)]) -> TempDir {
    let tmp = TempDir::new().unwrap();
    let schema_dir = tmp.path().join("schemas").join(name);
    let briefs_dir = schema_dir.join("briefs");
    std::fs::create_dir_all(&briefs_dir).unwrap();
    std::fs::write(schema_dir.join("adapter.yaml"), schema_yaml).unwrap();
    for (rel, contents) in briefs {
        let target = schema_dir.join(rel);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&target, contents).unwrap();
    }
    tmp
}

const VALID_SCHEMA_YAML: &str = "\
name: demo
version: 1
description: demo
pipeline:
  define:
    - { id: proposal, brief: briefs/proposal.md }
    - { id: specs,    brief: briefs/specs.md }
  build:
    - { id: build, brief: briefs/build.md }
  merge:
    - { id: merge, brief: briefs/merge.md }
";

fn valid_briefs() -> Vec<(&'static str, &'static str)> {
    vec![
        ("briefs/proposal.md", "---\nid: proposal\ndescription: why\n---\nbody\n"),
        ("briefs/specs.md", "---\nid: specs\ndescription: what\nneeds: [proposal]\n---\nbody\n"),
        (
            "briefs/build.md",
            "---\nid: build\ndescription: implement\nneeds: [specs]\ntracks: specs\n---\nbody\n",
        ),
        ("briefs/merge.md", "---\nid: merge\ndescription: land\nneeds: [build]\n---\nbody\n"),
    ]
}

const PLAN_SCHEMA_YAML: &str = "\
name: demo
version: 1
description: demo with plan
pipeline:
  plan:
    - { id: discovery, brief: briefs/plan/discovery.md }
    - { id: propose,   brief: briefs/plan/propose.md }
  define:
    - { id: proposal, brief: briefs/proposal.md }
    - { id: specs,    brief: briefs/specs.md }
  build:
    - { id: build, brief: briefs/build.md }
  merge:
    - { id: merge, brief: briefs/merge.md }
";

fn plan_briefs() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            "briefs/plan/discovery.md",
            "---\nid: discovery\ndescription: explore\ngenerates: discovery.md\n---\nbody\n",
        ),
        (
            "briefs/plan/propose.md",
            "---\nid: propose\ndescription: propose\nneeds: [discovery]\ngenerates: propose.md\n---\nbody\n",
        ),
        ("briefs/proposal.md", "---\nid: proposal\ndescription: why\n---\nbody\n"),
        ("briefs/specs.md", "---\nid: specs\ndescription: what\nneeds: [proposal]\n---\nbody\n"),
        (
            "briefs/build.md",
            "---\nid: build\ndescription: impl\nneeds: [specs]\ntracks: specs\n---\nbody\n",
        ),
        ("briefs/merge.md", "---\nid: merge\ndescription: land\nneeds: [build]\n---\nbody\n"),
    ]
}

#[test]
fn pipeline_view_load_includes_plan_briefs_in_topo_order() {
    let tmp = scaffold_schema_project("demo", PLAN_SCHEMA_YAML, &plan_briefs());
    let view = PipelineView::load("demo", tmp.path()).expect("loads with plan briefs");

    assert_eq!(view.phase(Phase::Plan).count(), 2);
    assert!(view.brief("discovery").is_some());
    assert!(view.brief("propose").is_some());

    // Topological order respects the plan-phase `needs: [discovery]` edge.
    let order: Vec<&str> = view
        .topo_order(Phase::Plan)
        .expect("plan topo order")
        .iter()
        .map(|b| b.frontmatter.id.as_str())
        .collect();
    assert_eq!(order, vec!["discovery", "propose"]);

    // Completion relative to an empty change dir: both briefs declare
    // `generates` and neither is present.
    let change_dir = tmp.path().join("change");
    std::fs::create_dir_all(&change_dir).unwrap();
    let completion = view.completion_for(Phase::Plan, &change_dir);
    assert_eq!(completion.get("discovery"), Some(&false));
    assert_eq!(completion.get("propose"), Some(&false));

    std::fs::write(change_dir.join("discovery.md"), "body").unwrap();
    let completion = view.completion_for(Phase::Plan, &change_dir);
    assert_eq!(completion.get("discovery"), Some(&true));
    assert_eq!(completion.get("propose"), Some(&false));
}

#[test]
fn pipeline_view_load_detects_id_mismatch() {
    let mut briefs = valid_briefs();
    briefs[0].1 = "---\nid: not-proposal\ndescription: wrong id\n---\nbody\n";
    let tmp = scaffold_schema_project("demo", VALID_SCHEMA_YAML, &briefs);

    let err = PipelineView::load("demo", tmp.path()).expect_err("id mismatch detected");
    match err {
        Error::Diag { detail: msg, .. } => {
            assert!(msg.contains("not-proposal"), "msg: {msg}");
            assert!(msg.contains("proposal"), "msg: {msg}");
        }
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn pipeline_view_load_rejects_needs_pointing_at_later_brief() {
    let mut briefs = valid_briefs();
    // proposal declares needs: [specs] — but specs is later in pipeline order.
    briefs[0].1 = "---\nid: proposal\ndescription: demo\nneeds: [specs]\n---\nbody\n";
    let tmp = scaffold_schema_project("demo", VALID_SCHEMA_YAML, &briefs);

    let err = PipelineView::load("demo", tmp.path()).expect_err("forward needs detected");
    match err {
        Error::Diag { detail: msg, .. } => {
            assert!(msg.contains("proposal"), "msg: {msg}");
            assert!(msg.contains("specs"), "msg: {msg}");
            assert!(msg.contains("earlier"), "msg: {msg}");
        }
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn pipeline_view_load_rejects_tracks_pointing_at_unknown_brief() {
    let mut briefs = valid_briefs();
    briefs[2].1 = "---\nid: build\ndescription: impl\nneeds: [specs]\ntracks: ghost\n---\nbody\n";
    let tmp = scaffold_schema_project("demo", VALID_SCHEMA_YAML, &briefs);

    let err = PipelineView::load("demo", tmp.path()).expect_err("unknown tracks detected");
    match err {
        Error::Diag { detail: msg, .. } => {
            assert!(msg.contains("ghost"), "msg: {msg}");
            assert!(msg.contains("build"), "msg: {msg}");
        }
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn schema_resolve_errors_when_url_schema_not_in_cache() {
    let tmp = TempDir::new().unwrap();
    let err = Adapter::resolve("https://example.com/schemas/nope", tmp.path())
        .expect_err("url with empty cache fails");
    match err {
        Error::Diag { detail: msg, .. } => assert!(msg.contains(".cache"), "msg: {msg}"),
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn schema_resolve_prefers_cache_over_local_for_bare_names() {
    let tmp = TempDir::new().unwrap();
    // Populate both: local `schemas/demo/` *and* `.specify/.cache/demo/`
    // with different descriptions. Cached wins.
    let local = tmp.path().join("schemas").join("demo");
    let cached = tmp.path().join(".specify").join(".cache").join("demo");
    std::fs::create_dir_all(local.join("briefs")).unwrap();
    std::fs::create_dir_all(cached.join("briefs")).unwrap();

    let local_yaml = VALID_SCHEMA_YAML.replace("description: demo", "description: local");
    let cached_yaml = VALID_SCHEMA_YAML.replace("description: demo", "description: cached");
    std::fs::write(local.join("adapter.yaml"), local_yaml).unwrap();
    std::fs::write(cached.join("adapter.yaml"), cached_yaml).unwrap();

    let resolved = Adapter::resolve("demo", tmp.path()).unwrap();
    assert!(matches!(resolved.source, AdapterSource::Cached(_)));
    assert_eq!(resolved.manifest.description, "cached");
}

// ---------- CacheMeta ----------

#[test]
fn cache_meta_load_returns_none_when_file_missing() {
    let tmp = TempDir::new().unwrap();
    let loaded = CacheMeta::load(tmp.path()).unwrap();
    assert!(loaded.is_none());
}

#[test]
fn cache_meta_load_roundtrip_and_malformed() {
    let tmp = TempDir::new().unwrap();
    let path = CacheMeta::path(tmp.path());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();

    let body = "schema_url: local:omnia\nfetched_at: 2026-04-17T00:00:00Z\n";
    std::fs::write(&path, body).unwrap();
    let meta = CacheMeta::load(tmp.path()).unwrap().expect("present");
    assert_eq!(meta.schema_url, "local:omnia");
    assert_eq!(meta.fetched_at, "2026-04-17T00:00:00Z");

    let results = meta.validate_structure();
    assert!(
        results.iter().all(|r| r.status == ValidationStatus::Pass),
        "expected valid, got {results:?}"
    );

    std::fs::write(&path, ": not: valid: yaml:\n\t-garbage").unwrap();
    let err = CacheMeta::load(tmp.path()).expect_err("malformed parse fails");
    assert!(matches!(err, Error::Diag { .. }), "got: {err:?}");
}

#[test]
fn cache_meta_matches_encodes_bare_and_url() {
    let bare = CacheMeta {
        schema_url: "local:omnia".into(),
        fetched_at: "2026-04-17T00:00:00Z".into(),
    };
    assert!(bare.matches("omnia"));
    assert!(!bare.matches("other"));
    assert!(!bare.matches("https://example.com/schemas/omnia"));

    let url = CacheMeta {
        schema_url: "https://example.com/schemas/omnia@v1".into(),
        fetched_at: "2026-04-17T00:00:00Z".into(),
    };
    assert!(url.matches("https://example.com/schemas/omnia@v1"));
    assert!(!url.matches("https://example.com/schemas/omnia"));
    assert!(!url.matches("omnia"));
}

#[test]
fn cache_meta_validate_structure_fails_on_empty_fields() {
    let meta = CacheMeta {
        schema_url: String::new(),
        fetched_at: String::new(),
    };
    let results = meta.validate_structure();
    assert!(
        results.iter().any(|r| r.status == ValidationStatus::Fail),
        "empty strings should fail minLength: {results:?}"
    );
}
