//! Unit tests for `specify-schema`.
//!
//! The "workspace" fixture we test against is the `specify` repo itself:
//! `schemas/omnia/` and `schemas/omnia/briefs/*.md` are real, hand-edited
//! files that every skill already relies on. By pointing
//! `PipelineView::load` at the repo root via `CARGO_MANIFEST_DIR`, we
//! parity-test the crate against those real inputs without checking in a
//! duplicated fixture tree.

use std::path::{Path, PathBuf};

use specify_error::Error;
use tempfile::TempDir;

use crate::ValidationResult;
use crate::brief::Brief;
use crate::cache::CacheMeta;
use crate::initiative_brief::{InitiativeBrief, InputKind};
use crate::pipeline::PipelineView;
use crate::registry::{ContractRoles, Registry, RegistryProject};
use crate::schema::{Phase, Schema, SchemaSource};

/// Absolute path to the repo root (the Cargo workspace root).
fn repo_root() -> PathBuf {
    // CARGO_MANIFEST_DIR = <repo>/crates/schema
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .and_then(|p| p.parent())
        .map(Path::to_path_buf)
        .expect("CARGO_MANIFEST_DIR should have two ancestors (crates/, repo root)")
}

fn omnia_schema_path() -> PathBuf {
    repo_root().join("schemas").join("omnia").join("schema.yaml")
}

// ---------- Schema parsing ----------

#[test]
fn parses_omnia_schema_yaml_fields_and_entries() {
    let raw = std::fs::read_to_string(omnia_schema_path()).expect("omnia schema on disk");
    let schema: Schema = serde_saphyr::from_str(&raw).expect("omnia schema is valid YAML");

    assert_eq!(schema.name, "omnia");
    assert_eq!(schema.version, 1);
    assert_eq!(schema.description, "Omnia Rust WASM workflow");
    assert!(schema.extends.is_none());
    let domain = schema.domain.as_deref().expect("omnia has a domain block");
    assert!(domain.contains("Rust, WASM"), "unexpected domain body: {domain:?}");

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
    let raw = std::fs::read_to_string(omnia_schema_path()).unwrap();
    let schema: Schema = serde_saphyr::from_str(&raw).unwrap();
    let results = schema.validate_structure();
    assert!(
        results.iter().all(|r| matches!(r, ValidationResult::Pass { .. })),
        "expected all passes, got: {results:?}"
    );
}

#[test]
fn validate_structure_fails_when_define_phase_is_empty() {
    let schema = Schema {
        name: "broken".into(),
        version: 1,
        description: "empty define phase".into(),
        extends: None,
        domain: None,
        pipeline: crate::schema::Pipeline {
            plan: vec![],
            define: vec![],
            build: vec![crate::schema::PipelineEntry {
                id: "build".into(),
                brief: "briefs/build.md".into(),
            }],
            merge: vec![crate::schema::PipelineEntry {
                id: "merge".into(),
                brief: "briefs/merge.md".into(),
            }],
        },
    };

    let results = schema.validate_structure();
    assert!(
        results.iter().any(|r| matches!(r, ValidationResult::Fail { .. })),
        "expected at least one failure, got: {results:?}"
    );
}

#[test]
fn yaml_parse_error_surface_for_missing_required_field() {
    // `description` missing -> serde error is propagated as an
    // `Error::Yaml` when surfaced through `Schema::resolve`, but here we
    // just exercise the parser directly and assert the Display message.
    let yaml = "name: broken\nversion: 1\npipeline:\n  define: []\n  build: []\n  merge: []\n";
    let err = serde_saphyr::from_str::<Schema>(yaml).expect_err("missing description");
    let message = err.to_string();
    assert!(
        message.contains("description"),
        "expected parse error to mention missing field, got: {message}"
    );
}

// ---------- pipeline.plan (Layer 3 authoring) ----------

#[test]
fn pipeline_plan_parses_when_present() {
    let yaml = r"
name: demo
version: 1
description: demo with plan
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
    let schema: Schema = serde_saphyr::from_str(yaml).expect("parses");
    let plan = schema.plan_entries();
    assert_eq!(plan.len(), 2);
    assert_eq!(plan[0].id, "discovery");
    assert_eq!(plan[0].brief, "briefs/plan/discovery.md");
    assert_eq!(plan[1].id, "propose");
    assert_eq!(plan[1].brief, "briefs/plan/propose.md");

    // `entries()` stays the execution loop only — plan briefs do not
    // leak into define/build/merge iteration.
    assert!(!schema.entries().any(|(p, _)| p == Phase::Plan));

    // Plan briefs are still discoverable via `entry()`.
    let (phase, entry) = schema.entry("discovery").expect("discovery visible via entry()");
    assert_eq!(phase, Phase::Plan);
    assert_eq!(entry.id, "discovery");

    // Structure validation accepts the schema end-to-end.
    let results = schema.validate_structure();
    assert!(
        results.iter().all(|r| matches!(r, ValidationResult::Pass { .. })),
        "plan-bearing schema should validate: {results:?}"
    );
}

#[test]
fn pipeline_without_plan_parses_unchanged() {
    let raw = std::fs::read_to_string(omnia_schema_path()).unwrap();
    let schema: Schema = serde_saphyr::from_str(&raw).unwrap();
    assert!(schema.pipeline.plan.is_empty());
    assert!(schema.plan_entries().is_empty());

    // Serializing back out must not introduce a `plan: []` key — we
    // skip-serialize empty plan vectors so round-trips of legacy
    // schemas are byte-stable for the plan field.
    let written = serde_saphyr::to_string(&schema).unwrap();
    assert!(
        !written.contains("plan:"),
        "expected no plan key in re-serialized omnia schema, got:\n{written}"
    );
}

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
    let parent: Schema = serde_saphyr::from_str(parent_yaml).unwrap();
    let child: Schema = serde_saphyr::from_str(child_yaml).unwrap();
    let merged = Schema::merge(parent, child);

    let ids: Vec<&str> = merged.plan_entries().iter().map(|e| e.id.as_str()).collect();
    assert_eq!(ids, vec!["discovery", "propose", "record"]);
    let propose = merged.plan_entries().iter().find(|e| e.id == "propose").unwrap();
    assert_eq!(propose.brief, "briefs/plan/propose-v2.md");
}

#[test]
fn json_schema_rejects_missing_define_even_with_plan_present() {
    // `pipeline.plan` is allowed but `define` is still required.
    let schema = Schema {
        name: "broken".into(),
        version: 1,
        description: "no define".into(),
        extends: None,
        domain: None,
        pipeline: crate::schema::Pipeline {
            plan: vec![crate::schema::PipelineEntry {
                id: "discovery".into(),
                brief: "briefs/plan/discovery.md".into(),
            }],
            define: vec![],
            build: vec![crate::schema::PipelineEntry {
                id: "build".into(),
                brief: "briefs/build.md".into(),
            }],
            merge: vec![crate::schema::PipelineEntry {
                id: "merge".into(),
                brief: "briefs/merge.md".into(),
            }],
        },
    };
    let results = schema.validate_structure();
    assert!(
        results.iter().any(|r| matches!(r, ValidationResult::Fail { .. })),
        "empty define must still fail even with plan present: {results:?}"
    );
}

// ---------- Composition via extends ----------

#[test]
fn merge_overrides_by_id_and_appends_new_entries() {
    let parent_yaml = r"
name: parent
version: 1
description: parent schema
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
description: child schema
pipeline:
  define:
    - { id: specs,   brief: briefs/specs-v2.md }
    - { id: review,  brief: briefs/review.md }
  build:
    - { id: build, brief: briefs/build.md }
  merge:
    - { id: merge, brief: briefs/merge.md }
";

    let parent: Schema = serde_saphyr::from_str(parent_yaml).unwrap();
    let child: Schema = serde_saphyr::from_str(child_yaml).unwrap();
    let merged = Schema::merge(parent, child);

    assert_eq!(merged.name, "child");
    assert_eq!(merged.version, 2);
    assert!(merged.extends.is_none(), "extends should be cleared");

    let ids: Vec<&str> = merged.pipeline.define.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(ids, vec!["proposal", "specs", "review"]);

    let specs = &merged.pipeline.define.iter().find(|e| e.id == "specs").unwrap().brief;
    assert_eq!(specs, "briefs/specs-v2.md", "child override took effect");
}

// ---------- entries / entry ----------

#[test]
fn entries_iterates_in_phase_order_and_entry_lookup_works() {
    let raw = std::fs::read_to_string(omnia_schema_path()).unwrap();
    let schema: Schema = serde_saphyr::from_str(&raw).unwrap();

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
    let raw = std::fs::read_to_string(omnia_schema_path()).unwrap();
    let schema: Schema = serde_saphyr::from_str(&raw).unwrap();
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
    assert!(matches!(err, Error::Config(_)), "got: {err:?}");
}

#[test]
fn brief_parse_rejects_missing_closing_delimiter() {
    let path = PathBuf::from("unclosed.md");
    let contents = "---\nid: proposal\ndescription: demo\n";
    let err = Brief::parse(&path, contents).expect_err("missing closing ---");
    match err {
        Error::Config(msg) => assert!(msg.contains("closing `---`"), "msg: {msg}"),
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn brief_parse_rejects_invalid_yaml_frontmatter() {
    let path = PathBuf::from("bad-yaml.md");
    let contents = "---\nid: [unclosed\n---\nbody\n";
    let err = Brief::parse(&path, contents).expect_err("malformed YAML");
    assert!(matches!(err, Error::Config(_)), "got: {err:?}");
}

// ---------- PipelineView ----------

#[test]
fn pipeline_view_loads_omnia_schema_from_workspace() {
    let root = repo_root();
    let view = PipelineView::load("omnia", &root).expect("omnia view loads");
    assert_eq!(view.briefs.len(), 6);
    assert!(matches!(view.schema.source, SchemaSource::Local(_)));

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

/// Scaffold a minimal local schema at `<project>/schemas/<name>/` with
/// the given `schema.yaml` and brief contents. Each brief content map
/// entry is `(filename, contents)` written under `schemas/<name>/`.
fn scaffold_schema_project(name: &str, schema_yaml: &str, briefs: &[(&str, &str)]) -> TempDir {
    let tmp = TempDir::new().unwrap();
    let schema_dir = tmp.path().join("schemas").join(name);
    let briefs_dir = schema_dir.join("briefs");
    std::fs::create_dir_all(&briefs_dir).unwrap();
    std::fs::write(schema_dir.join("schema.yaml"), schema_yaml).unwrap();
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
        Error::SchemaResolution(msg) => {
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
        Error::SchemaResolution(msg) => {
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
        Error::SchemaResolution(msg) => {
            assert!(msg.contains("ghost"), "msg: {msg}");
            assert!(msg.contains("build"), "msg: {msg}");
        }
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn schema_resolve_errors_when_url_schema_not_in_cache() {
    let tmp = TempDir::new().unwrap();
    let err = Schema::resolve("https://example.com/schemas/nope", tmp.path())
        .expect_err("url with empty cache fails");
    match err {
        Error::SchemaResolution(msg) => assert!(msg.contains(".cache"), "msg: {msg}"),
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
    std::fs::write(local.join("schema.yaml"), local_yaml).unwrap();
    std::fs::write(cached.join("schema.yaml"), cached_yaml).unwrap();

    let resolved = Schema::resolve("demo", tmp.path()).unwrap();
    assert!(matches!(resolved.source, SchemaSource::Cached(_)));
    assert_eq!(resolved.schema.description, "cached");
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
        results.iter().all(|r| matches!(r, ValidationResult::Pass { .. })),
        "expected valid, got {results:?}"
    );

    std::fs::write(&path, ": not: valid: yaml:\n\t-garbage").unwrap();
    let err = CacheMeta::load(tmp.path()).expect_err("malformed parse fails");
    assert!(matches!(err, Error::Config(_)), "got: {err:?}");
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
        results.iter().any(|r| matches!(r, ValidationResult::Fail { .. })),
        "empty strings should fail minLength: {results:?}"
    );
}

// ---------- Registry (RFC-3a §"The Registry") ----------

/// Scaffold `.specify/registry.yaml` with `contents` and return the
/// containing project directory.
fn scaffold_registry(contents: &str) -> TempDir {
    let tmp = TempDir::new().unwrap();
    let specify_dir = tmp.path().join(".specify");
    std::fs::create_dir_all(&specify_dir).unwrap();
    std::fs::write(Registry::path(tmp.path()), contents).unwrap();
    tmp
}

const CANONICAL_REGISTRY_YAML: &str = "\
version: 1
projects:
  - name: traffic
    url: .
    schema: omnia@v1
";

const MULTI_PROJECT_REGISTRY_YAML: &str = "\
version: 1
projects:
  - name: traffic
    url: .
    schema: omnia@v1
    description: Real-time traffic routing service
  - name: ingest
    url: git@github.com:augentic/ingest.git
    schema: omnia@v1
    description: Data ingestion pipeline
  - name: ops-runbook
    url: https://github.com/augentic/ops-runbook
    schema: omnia@v1
    description: Operational runbook reference
";

#[test]
fn registry_absent_returns_none() {
    let tmp = TempDir::new().unwrap();
    let loaded = Registry::load(tmp.path()).expect("absent registry is not an error");
    assert!(loaded.is_none());
}

#[test]
fn registry_parses_canonical_rfc_example() {
    let tmp = scaffold_registry(CANONICAL_REGISTRY_YAML);
    let registry = Registry::load(tmp.path()).expect("parses").expect("present");
    assert_eq!(registry.version, 1);
    assert_eq!(registry.projects.len(), 1);
    assert_eq!(registry.projects[0].name, "traffic");
    assert_eq!(registry.projects[0].url, ".");
    assert_eq!(registry.projects[0].schema, "omnia@v1");
}

#[test]
fn registry_parses_multi_project() {
    let tmp = scaffold_registry(MULTI_PROJECT_REGISTRY_YAML);
    let registry = Registry::load(tmp.path()).expect("parses").expect("present");
    let round_tripped_yaml = serde_saphyr::to_string(&registry).unwrap();
    let re_parsed: Registry = serde_saphyr::from_str(&round_tripped_yaml).unwrap();
    assert_eq!(registry, re_parsed);
}

#[test]
fn registry_rejects_unknown_top_level_key() {
    let yaml = "\
version: 1
foo: bar
projects: []
";
    let tmp = scaffold_registry(yaml);
    let err = Registry::load(tmp.path()).expect_err("unknown top-level key");
    match err {
        Error::Config(msg) => {
            assert!(msg.contains("foo"), "msg: {msg}");
        }
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn registry_rejects_unknown_project_key() {
    let yaml = "\
version: 1
projects:
  - name: traffic
    url: .
    schema: omnia@v1
    foo: bar
";
    let tmp = scaffold_registry(yaml);
    let err = Registry::load(tmp.path()).expect_err("unknown project key");
    match err {
        Error::Config(msg) => {
            assert!(msg.contains("foo"), "msg: {msg}");
        }
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn registry_rejects_version_not_one() {
    let yaml = "\
version: 2
projects: []
";
    let tmp = scaffold_registry(yaml);
    let err = Registry::load(tmp.path()).expect_err("version != 1");
    match err {
        Error::Config(msg) => {
            assert!(msg.contains("version"), "msg should mention version: {msg}");
            assert!(msg.contains('2'), "msg should mention the offending value: {msg}");
        }
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn registry_rejects_missing_version() {
    let yaml = "projects: []\n";
    let tmp = scaffold_registry(yaml);
    let err = Registry::load(tmp.path()).expect_err("missing version");
    match err {
        Error::Config(msg) => assert!(msg.contains("version"), "msg: {msg}"),
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn registry_rejects_missing_name() {
    let yaml = "\
version: 1
projects:
  - url: .
    schema: omnia@v1
";
    let tmp = scaffold_registry(yaml);
    let err = Registry::load(tmp.path()).expect_err("missing name");
    assert!(matches!(err, Error::Config(_)), "got: {err:?}");
}

#[test]
fn registry_rejects_missing_url() {
    let yaml = "\
version: 1
projects:
  - name: traffic
    schema: omnia@v1
";
    let tmp = scaffold_registry(yaml);
    let err = Registry::load(tmp.path()).expect_err("missing url");
    assert!(matches!(err, Error::Config(_)), "got: {err:?}");
}

#[test]
fn registry_rejects_missing_schema() {
    let yaml = "\
version: 1
projects:
  - name: traffic
    url: .
";
    let tmp = scaffold_registry(yaml);
    let err = Registry::load(tmp.path()).expect_err("missing schema");
    assert!(matches!(err, Error::Config(_)), "got: {err:?}");
}

#[test]
fn registry_rejects_non_kebab_case_name() {
    for bad in ["TrafficSystem", "traffic_system", "traffic--system", "-traffic", "traffic-"] {
        let yaml =
            format!("version: 1\nprojects:\n  - name: {bad}\n    url: .\n    schema: omnia@v1\n");
        let tmp = scaffold_registry(&yaml);
        let err = Registry::load(tmp.path()).expect_err(&format!("bad name `{bad}`"));
        match err {
            Error::Config(msg) => {
                assert!(msg.contains("kebab-case"), "msg for `{bad}`: {msg}");
                assert!(msg.contains(bad), "msg for `{bad}`: {msg}");
            }
            other => panic!("wrong variant for `{bad}`: {other:?}"),
        }
    }
}

#[test]
fn registry_rejects_empty_string_name() {
    let yaml = "\
version: 1
projects:
  - name: \"\"
    url: .
    schema: omnia@v1
";
    let tmp = scaffold_registry(yaml);
    let err = Registry::load(tmp.path()).expect_err("empty name");
    match err {
        Error::Config(msg) => {
            assert!(msg.contains("empty") || msg.contains("kebab-case"), "msg: {msg}");
        }
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn registry_rejects_empty_string_url() {
    let yaml = "\
version: 1
projects:
  - name: traffic
    url: \"\"
    schema: omnia@v1
";
    let tmp = scaffold_registry(yaml);
    let err = Registry::load(tmp.path()).expect_err("empty url");
    match err {
        Error::Config(msg) => assert!(msg.contains("url"), "msg: {msg}"),
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn registry_rejects_empty_string_schema() {
    let yaml = "\
version: 1
projects:
  - name: traffic
    url: .
    schema: \"\"
";
    let tmp = scaffold_registry(yaml);
    let err = Registry::load(tmp.path()).expect_err("empty schema");
    match err {
        Error::Config(msg) => assert!(msg.contains("schema"), "msg: {msg}"),
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn registry_rejects_duplicate_project_names() {
    let yaml = "\
version: 1
projects:
  - name: traffic
    url: .
    schema: omnia@v1
  - name: traffic
    url: ../other
    schema: omnia@v1
";
    let tmp = scaffold_registry(yaml);
    let err = Registry::load(tmp.path()).expect_err("duplicate name");
    match err {
        Error::Config(msg) => {
            assert!(msg.contains("duplicate"), "msg: {msg}");
            assert!(msg.contains("traffic"), "msg: {msg}");
        }
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn registry_accepts_empty_projects_list() {
    let yaml = "version: 1\nprojects: []\n";
    let tmp = scaffold_registry(yaml);
    let registry = Registry::load(tmp.path()).expect("parses").expect("present");
    assert!(registry.projects.is_empty());
    assert!(registry.is_single_repo());
}

#[test]
fn registry_accepts_single_project_and_is_single_repo() {
    let tmp = scaffold_registry(CANONICAL_REGISTRY_YAML);
    let registry = Registry::load(tmp.path()).unwrap().unwrap();
    assert_eq!(registry.projects.len(), 1);
    assert!(registry.is_single_repo());
}

#[test]
fn registry_accepts_multi_project_and_is_single_repo_false() {
    let tmp = scaffold_registry(MULTI_PROJECT_REGISTRY_YAML);
    let registry = Registry::load(tmp.path()).unwrap().unwrap();
    assert_eq!(registry.projects.len(), 3);
    assert!(!registry.is_single_repo());
}

#[test]
fn registry_round_trip_serialize() {
    let original = Registry {
        version: 1,
        projects: vec![
            RegistryProject {
                name: "traffic".into(),
                url: ".".into(),
                schema: "omnia@v1".into(),
                description: Some("Real-time traffic routing".into()),
                contracts: None,
            },
            RegistryProject {
                name: "ingest".into(),
                url: "git@github.com:augentic/ingest.git".into(),
                schema: "omnia@v1".into(),
                description: Some("Data ingestion pipeline".into()),
                contracts: None,
            },
        ],
    };
    let yaml = serde_saphyr::to_string(&original).expect("serialize");
    let round_tripped: Registry = serde_saphyr::from_str(&yaml).expect("re-parse");
    assert_eq!(round_tripped, original);
    round_tripped.validate_shape().expect("valid shape");
}

#[test]
fn registry_project_order_preserved() {
    let tmp = scaffold_registry(MULTI_PROJECT_REGISTRY_YAML);
    let registry = Registry::load(tmp.path()).unwrap().unwrap();
    let names: Vec<&str> = registry.projects.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, vec!["traffic", "ingest", "ops-runbook"]);
}

#[test]
fn registry_multi_project_with_descriptions_validates() {
    let yaml = "\
version: 1
projects:
  - name: alpha
    url: .
    schema: omnia@v1
    description: The alpha service
  - name: beta
    url: ../beta
    schema: omnia@v1
    description: The beta service
";
    let tmp = scaffold_registry(yaml);
    let registry = Registry::load(tmp.path()).expect("parses").expect("present");
    assert_eq!(registry.projects.len(), 2);
    assert_eq!(registry.projects[0].description.as_deref(), Some("The alpha service"));
    assert_eq!(registry.projects[1].description.as_deref(), Some("The beta service"));
}

#[test]
fn registry_multi_project_missing_description_rejected() {
    let yaml = "\
version: 1
projects:
  - name: alpha
    url: .
    schema: omnia@v1
    description: The alpha service
  - name: beta
    url: ../beta
    schema: omnia@v1
";
    let tmp = scaffold_registry(yaml);
    let err = Registry::load(tmp.path()).expect_err("missing description in multi-project");
    match err {
        Error::Config(msg) => {
            assert!(msg.contains("description-missing-multi-repo"), "msg: {msg}");
            assert!(msg.contains("beta"), "msg should mention project name: {msg}");
        }
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn registry_multi_project_empty_description_rejected() {
    let yaml = "\
version: 1
projects:
  - name: alpha
    url: .
    schema: omnia@v1
    description: \"  \"
  - name: beta
    url: ../beta
    schema: omnia@v1
    description: The beta service
";
    let tmp = scaffold_registry(yaml);
    let err = Registry::load(tmp.path()).expect_err("whitespace-only description in multi-project");
    match err {
        Error::Config(msg) => {
            assert!(msg.contains("description-missing-multi-repo"), "msg: {msg}");
            assert!(msg.contains("alpha"), "msg should mention project name: {msg}");
        }
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn registry_single_project_without_description_ok() {
    let tmp = scaffold_registry(CANONICAL_REGISTRY_YAML);
    let registry = Registry::load(tmp.path()).expect("parses").expect("present");
    assert_eq!(registry.projects.len(), 1);
    assert!(registry.projects[0].description.is_none());
}

#[test]
fn registry_description_round_trips_through_serde() {
    let original = RegistryProject {
        name: "traffic".into(),
        url: ".".into(),
        schema: "omnia@v1".into(),
        description: Some("Real-time traffic routing".into()),
        contracts: None,
    };
    let yaml = serde_saphyr::to_string(&original).expect("serialize");
    let round_tripped: RegistryProject = serde_saphyr::from_str(&yaml).expect("re-parse");
    assert_eq!(round_tripped, original);
}

#[test]
fn registry_path_helper_points_at_specify_dir() {
    let dir = Path::new("/tmp/some/project");
    assert_eq!(Registry::path(dir), PathBuf::from("/tmp/some/project/.specify/registry.yaml"));
}

// ---------- Registry URL validation (RFC-3a C28) ----------

fn registry_with_one_url(url: &str) -> Registry {
    Registry {
        version: 1,
        projects: vec![RegistryProject {
            name: "traffic".into(),
            url: url.into(),
            schema: "omnia@v1".into(),
            description: None,
            contracts: None,
        }],
    }
}

#[test]
fn registry_project_url_materialises_as_symlink_classification() {
    for (url, symlink) in [
        (".", true),
        ("../peer", true),
        ("./foo", true),
        ("pkg/sub", true),
        ("git@github.com:augentic/ingest.git", false),
        ("https://github.com/augentic/ops-runbook", false),
        ("http://example.com/repo.git", false),
        ("ssh://git@github.com/augentic/specify.git", false),
        ("git+https://example.com/org/repo.git", false),
        ("git+http://example.com/org/repo.git", false),
        ("git+ssh://git@github.com/org/repo.git", false),
    ] {
        let p = RegistryProject {
            name: "traffic".into(),
            url: url.into(),
            schema: "omnia@v1".into(),
            description: None,
            contracts: None,
        };
        assert_eq!(p.url_materialises_as_symlink(), symlink, "url={url:?}");
    }
}

#[test]
fn registry_accepts_url_shapes_for_c28() {
    for url in [
        "https://github.com/a/b",
        "http://github.com/a/b",
        "git@github.com:org/repo.git",
        "ssh://git@github.com/org/repo.git",
        "git+https://github.com/org/repo.git",
        "../peer-repo",
        "./inputs/legacy",
        "inputs/runbook",
    ] {
        registry_with_one_url(url).validate_shape().unwrap_or_else(|e| {
            panic!("expected url {url:?} to validate, got: {e}");
        });
    }
}

#[test]
fn registry_rejects_unsupported_url_scheme() {
    let err = registry_with_one_url("ftp://example.com/repo")
        .validate_shape()
        .expect_err("ftp must be rejected");
    match err {
        Error::Config(msg) => {
            assert!(msg.contains("ftp"), "msg: {msg}");
            assert!(msg.contains("scheme"), "msg: {msg}");
        }
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn registry_rejects_file_url_scheme() {
    let err = registry_with_one_url("file:///tmp/repo")
        .validate_shape()
        .expect_err("file:// must be rejected");
    assert!(matches!(err, Error::Config(_)), "got: {err:?}");
}

#[test]
fn registry_rejects_colon_without_scheme_or_git_at() {
    let err = registry_with_one_url("weird:path")
        .validate_shape()
        .expect_err("colon form must be rejected");
    match err {
        Error::Config(msg) => assert!(msg.contains(':'), "msg: {msg}"),
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn registry_rejects_absolute_unix_path_as_url() {
    let err = registry_with_one_url("/absolute/path")
        .validate_shape()
        .expect_err("absolute path must be rejected");
    match err {
        Error::Config(msg) => assert!(msg.contains("relative"), "msg: {msg}"),
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn registry_rejects_whitespace_only_url() {
    let err =
        registry_with_one_url("   ").validate_shape().expect_err("whitespace url must be rejected");
    match err {
        Error::Config(msg) => assert!(msg.contains("whitespace"), "msg: {msg}"),
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn registry_rejects_url_with_leading_whitespace() {
    let err = registry_with_one_url(" https://example.com/a")
        .validate_shape()
        .expect_err("leading space must be rejected");
    match err {
        Error::Config(msg) => assert!(msg.contains("whitespace"), "msg: {msg}"),
        other => panic!("wrong variant: {other:?}"),
    }
}

// ---------- Registry hub-mode validation (RFC-9 §1D) ----------

#[test]
fn registry_validate_shape_hub_accepts_empty_projects() {
    let reg = Registry {
        version: 1,
        projects: vec![],
    };
    reg.validate_shape_hub().expect("empty hub registry must pass");
}

#[test]
fn registry_validate_shape_hub_accepts_non_dot_urls() {
    let reg = Registry {
        version: 1,
        projects: vec![
            RegistryProject {
                name: "alpha".into(),
                url: "git@github.com:augentic/alpha.git".into(),
                schema: "omnia@v1".into(),
                description: Some("Alpha service".into()),
                contracts: None,
            },
            RegistryProject {
                name: "beta".into(),
                url: "../beta".into(),
                schema: "omnia@v1".into(),
                description: Some("Beta service".into()),
                contracts: None,
            },
        ],
    };
    reg.validate_shape_hub().expect("non-`.` urls must pass hub-mode validation");
}

#[test]
fn registry_validate_shape_hub_rejects_dot_url_entry() {
    let reg = Registry {
        version: 1,
        projects: vec![RegistryProject {
            name: "platform".into(),
            url: ".".into(),
            schema: "omnia@v1".into(),
            description: None,
            contracts: None,
        }],
    };
    let err = reg.validate_shape_hub().expect_err("hub mode must reject url: .");
    match err {
        Error::Config(msg) => {
            assert!(
                msg.contains("hub-cannot-be-project"),
                "diagnostic must carry the stable code, got: {msg}"
            );
            assert!(msg.contains("platform"), "diagnostic must name the offending project: {msg}");
            assert!(msg.contains("registry.yaml"), "diagnostic must scope the file: {msg}");
        }
        other => panic!("wrong error variant: {other:?}"),
    }
}

#[test]
fn registry_validate_shape_hub_rejects_dot_url_in_multi_project() {
    let reg = Registry {
        version: 1,
        projects: vec![
            RegistryProject {
                name: "alpha".into(),
                url: "../alpha".into(),
                schema: "omnia@v1".into(),
                description: Some("Alpha service".into()),
                contracts: None,
            },
            RegistryProject {
                name: "self-as-project".into(),
                url: ".".into(),
                schema: "omnia@v1".into(),
                description: Some("Should be the hub, not an entry".into()),
                contracts: None,
            },
        ],
    };
    let err = reg.validate_shape_hub().expect_err("hub mode rejects `.` even alongside peers");
    match err {
        Error::Config(msg) => {
            assert!(msg.contains("hub-cannot-be-project"), "msg: {msg}");
            assert!(msg.contains("self-as-project"), "msg should name the offender: {msg}");
        }
        other => panic!("wrong error variant: {other:?}"),
    }
}

#[test]
fn registry_validate_shape_hub_inherits_base_shape_errors() {
    // version != 1 is a base-shape error; hub mode must surface it
    // without ever reaching the `hub-cannot-be-project` check.
    let reg = Registry {
        version: 2,
        projects: vec![],
    };
    let err = reg.validate_shape_hub().expect_err("base shape error must propagate through");
    match err {
        Error::Config(msg) => {
            assert!(msg.contains("version"), "msg: {msg}");
            assert!(
                !msg.contains("hub-cannot-be-project"),
                "must not short-circuit base-shape errors with the hub diagnostic: {msg}"
            );
        }
        other => panic!("wrong error variant: {other:?}"),
    }
}

#[test]
fn registry_validate_shape_unchanged_for_dot_url() {
    // The base `validate_shape` continues to accept `url: .` — only
    // the new hub-only mode rejects it. This pins the additive-API
    // contract from the RFC.
    let reg = Registry {
        version: 1,
        projects: vec![RegistryProject {
            name: "platform".into(),
            url: ".".into(),
            schema: "omnia@v1".into(),
            description: None,
            contracts: None,
        }],
    };
    reg.validate_shape().expect("base shape must still accept `url: .`");
}

// ---------- Registry contract roles (RFC-8 Layer 2) ----------

const REGISTRY_WITH_CONTRACT_ROLES_YAML: &str = "\
version: 1
projects:
  - name: traffic
    url: .
    schema: omnia@v1
    description: Real-time traffic routing service
    contracts:
      produces:
        - http/traffic-api.yaml
      consumes:
        - http/ingest-api.yaml
  - name: ingest
    url: git@github.com:augentic/ingest.git
    schema: omnia@v1
    description: Data ingestion pipeline
    contracts:
      produces:
        - http/ingest-api.yaml
      consumes:
        - schemas/order-placed.yaml
";

#[test]
fn registry_with_contract_roles_parses_and_validates() {
    let tmp = scaffold_registry(REGISTRY_WITH_CONTRACT_ROLES_YAML);
    let registry = Registry::load(tmp.path()).expect("parses").expect("present");
    assert_eq!(registry.projects.len(), 2);

    let traffic = &registry.projects[0];
    let roles = traffic.contracts.as_ref().expect("traffic has contracts");
    assert_eq!(roles.produces, vec!["http/traffic-api.yaml"]);
    assert_eq!(roles.consumes, vec!["http/ingest-api.yaml"]);

    let ingest = &registry.projects[1];
    let roles = ingest.contracts.as_ref().expect("ingest has contracts");
    assert_eq!(roles.produces, vec!["http/ingest-api.yaml"]);
    assert_eq!(roles.consumes, vec!["schemas/order-placed.yaml"]);
}

#[test]
fn registry_without_contract_roles_still_parses() {
    let tmp = scaffold_registry(MULTI_PROJECT_REGISTRY_YAML);
    let registry = Registry::load(tmp.path()).expect("parses").expect("present");
    for project in &registry.projects {
        assert!(project.contracts.is_none());
    }
}

#[test]
fn registry_contract_roles_round_trip_omits_empty_fields() {
    let original = Registry {
        version: 1,
        projects: vec![RegistryProject {
            name: "traffic".into(),
            url: ".".into(),
            schema: "omnia@v1".into(),
            description: None,
            contracts: Some(ContractRoles {
                produces: vec!["http/traffic-api.yaml".into()],
                consumes: vec![],
            }),
        }],
    };
    let yaml = serde_saphyr::to_string(&original).expect("serialize");
    assert!(!yaml.contains("consumes"), "empty consumes should be omitted: {yaml}");
    let round_tripped: Registry = serde_saphyr::from_str(&yaml).expect("re-parse");
    assert_eq!(round_tripped, original);
}

#[test]
fn registry_contract_roles_none_omits_contracts_key() {
    let original = Registry {
        version: 1,
        projects: vec![RegistryProject {
            name: "traffic".into(),
            url: ".".into(),
            schema: "omnia@v1".into(),
            description: None,
            contracts: None,
        }],
    };
    let yaml = serde_saphyr::to_string(&original).expect("serialize");
    assert!(!yaml.contains("contracts"), "None contracts should be omitted: {yaml}");
}

#[test]
fn registry_rejects_single_producer_violation() {
    let yaml = "\
version: 1
projects:
  - name: alpha
    url: .
    schema: omnia@v1
    description: Alpha service
    contracts:
      produces:
        - http/shared-api.yaml
  - name: beta
    url: ../beta
    schema: omnia@v1
    description: Beta service
    contracts:
      produces:
        - http/shared-api.yaml
";
    let tmp = scaffold_registry(yaml);
    let err = Registry::load(tmp.path()).expect_err("single producer violation");
    match err {
        Error::Config(msg) => {
            assert!(msg.contains("http/shared-api.yaml"), "msg: {msg}");
            assert!(msg.contains("produced by both"), "msg: {msg}");
        }
        other => panic!("wrong variant: {other:?}"),
    }
}

/// RFC-12 dropped `contracts.imports`. Any registry that still
/// declares the field after the upgrade fails fast at parse time
/// (`#[serde(deny_unknown_fields)]`) — that diagnostic is the
/// documented migration trigger from RFC-12 §Migration.
#[test]
fn registry_rejects_unknown_imports_field() {
    let yaml = "\
version: 1
projects:
  - name: alpha
    url: .
    schema: omnia@v1
    contracts:
      imports:
        - http/external-api.yaml
";
    let tmp = scaffold_registry(yaml);
    let err = Registry::load(tmp.path()).expect_err("legacy imports field rejected");
    match err {
        Error::Config(msg) => assert!(msg.contains("imports"), "msg: {msg}"),
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn registry_rejects_absolute_path_in_contract_role() {
    let yaml = "\
version: 1
projects:
  - name: alpha
    url: .
    schema: omnia@v1
    contracts:
      produces:
        - /absolute/path.yaml
";
    let tmp = scaffold_registry(yaml);
    let err = Registry::load(tmp.path()).expect_err("absolute path rejected");
    match err {
        Error::Config(msg) => {
            assert!(msg.contains("/absolute/path.yaml"), "msg: {msg}");
            assert!(msg.contains("relative"), "msg: {msg}");
        }
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn registry_rejects_dotdot_in_contract_path() {
    let yaml = "\
version: 1
projects:
  - name: alpha
    url: .
    schema: omnia@v1
    contracts:
      consumes:
        - ../escape/path.yaml
";
    let tmp = scaffold_registry(yaml);
    let err = Registry::load(tmp.path()).expect_err(".. path rejected");
    match err {
        Error::Config(msg) => {
            assert!(msg.contains("../escape/path.yaml"), "msg: {msg}");
            assert!(msg.contains("relative"), "msg: {msg}");
        }
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn registry_rejects_self_consistency_violation() {
    let yaml = "\
version: 1
projects:
  - name: alpha
    url: .
    schema: omnia@v1
    contracts:
      produces:
        - http/my-api.yaml
      consumes:
        - http/my-api.yaml
";
    let tmp = scaffold_registry(yaml);
    let err = Registry::load(tmp.path()).expect_err("self-consistency violation");
    match err {
        Error::Config(msg) => {
            assert!(msg.contains("alpha"), "msg: {msg}");
            assert!(msg.contains("http/my-api.yaml"), "msg: {msg}");
            assert!(msg.contains("produces"), "msg: {msg}");
            assert!(msg.contains("consumes"), "msg: {msg}");
        }
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn registry_rejects_unknown_contract_roles_key() {
    let yaml = "\
version: 1
projects:
  - name: alpha
    url: .
    schema: omnia@v1
    contracts:
      produces:
        - http/api.yaml
      bogus:
        - something
";
    let tmp = scaffold_registry(yaml);
    let err = Registry::load(tmp.path()).expect_err("unknown contract key");
    match err {
        Error::Config(msg) => assert!(msg.contains("bogus"), "msg: {msg}"),
        other => panic!("wrong variant: {other:?}"),
    }
}

// ---------- Initiative brief (RFC-3a §"The Initiative Brief") ----------

/// Scaffold `.specify/initiative.md` with `contents` and return the
/// containing project directory.
fn scaffold_initiative_brief(contents: &str) -> TempDir {
    let tmp = TempDir::new().unwrap();
    let specify_dir = tmp.path().join(".specify");
    std::fs::create_dir_all(&specify_dir).unwrap();
    std::fs::write(InitiativeBrief::path(tmp.path()), contents).unwrap();
    tmp
}

/// Byte-for-byte golden for [`InitiativeBrief::template`] applied to
/// the RFC's `traffic-modernisation` example. The CLI test in
/// `tests/initiative.rs` pins the exact same bytes against
/// `specify initiative brief init traffic-modernisation`.
const TRAFFIC_TEMPLATE_GOLDEN: &str = "\
---
name: traffic-modernisation
inputs: []
---

# Traffic modernisation

<!-- One-paragraph framing of what this initiative is trying to
     achieve. Plans reference this brief via `.specify/initiative.md`. -->
";

/// The RFC's canonical example, with frontmatter inputs + prose body.
const CANONICAL_INITIATIVE_MD: &str = "\
---
name: traffic-modernisation
inputs:
  - path: ./inputs/legacy-traffic/
    kind: legacy-code
  - path: ./inputs/ops-runbook.pdf
    kind: documentation
---

# Traffic modernisation

Move the legacy traffic system onto Omnia, preserving…
";

#[test]
fn initiative_brief_absent_returns_none() {
    let tmp = TempDir::new().unwrap();
    let loaded = InitiativeBrief::load(tmp.path()).expect("absent is not an error");
    assert!(loaded.is_none());
}

#[test]
fn initiative_brief_parses_canonical_rfc_example() {
    let tmp = scaffold_initiative_brief(CANONICAL_INITIATIVE_MD);
    let brief = InitiativeBrief::load(tmp.path()).expect("parses").expect("present");

    assert_eq!(brief.frontmatter.name, "traffic-modernisation");
    assert_eq!(brief.frontmatter.inputs.len(), 2);
    assert_eq!(brief.frontmatter.inputs[0].path, "./inputs/legacy-traffic/");
    assert_eq!(brief.frontmatter.inputs[0].kind, InputKind::LegacyCode);
    assert_eq!(brief.frontmatter.inputs[1].path, "./inputs/ops-runbook.pdf");
    assert_eq!(brief.frontmatter.inputs[1].kind, InputKind::Documentation);

    assert_eq!(
        brief.body,
        "\n# Traffic modernisation\n\nMove the legacy traffic system onto Omnia, preserving…\n"
    );
}

#[test]
fn initiative_brief_parses_no_inputs() {
    let yaml = "---\nname: solo\n---\n\n# Solo\n\nA brief without an inputs key.\n";
    let tmp = scaffold_initiative_brief(yaml);
    let brief = InitiativeBrief::load(tmp.path()).unwrap().unwrap();
    assert_eq!(brief.frontmatter.name, "solo");
    assert!(brief.frontmatter.inputs.is_empty());
    assert_eq!(brief.body, "\n# Solo\n\nA brief without an inputs key.\n");
}

#[test]
fn initiative_brief_parses_empty_inputs() {
    let yaml = "---\nname: solo\ninputs: []\n---\n\nbody\n";
    let tmp = scaffold_initiative_brief(yaml);
    let brief = InitiativeBrief::load(tmp.path()).unwrap().unwrap();
    assert!(brief.frontmatter.inputs.is_empty());
}

#[test]
fn initiative_brief_rejects_missing_frontmatter() {
    let tmp = scaffold_initiative_brief("# Just markdown, no frontmatter\n");
    let err = InitiativeBrief::load(tmp.path()).expect_err("missing frontmatter");
    match err {
        Error::Config(msg) => {
            assert!(msg.contains("initiative.md"), "msg: {msg}");
            assert!(msg.contains("frontmatter"), "msg: {msg}");
        }
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn initiative_brief_rejects_unclosed_frontmatter() {
    let tmp = scaffold_initiative_brief("---\nname: solo\n# body has no closing ---\n");
    let err = InitiativeBrief::load(tmp.path()).expect_err("no closing delimiter");
    match err {
        Error::Config(msg) => assert!(msg.contains("closing"), "msg: {msg}"),
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn initiative_brief_rejects_missing_name() {
    let tmp = scaffold_initiative_brief("---\ninputs: []\n---\n\nbody\n");
    let err = InitiativeBrief::load(tmp.path()).expect_err("missing name");
    match err {
        Error::Config(msg) => assert!(msg.contains("name"), "msg: {msg}"),
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn initiative_brief_rejects_non_kebab_name() {
    for bad in ["TrafficModern", "traffic_modern", "traffic--modern", "-bad", "bad-"] {
        let yaml = format!("---\nname: {bad}\n---\n\nbody\n");
        let tmp = scaffold_initiative_brief(&yaml);
        let err = InitiativeBrief::load(tmp.path()).expect_err(&format!("bad name `{bad}`"));
        match err {
            Error::Config(msg) => {
                assert!(msg.contains("kebab-case"), "msg for `{bad}`: {msg}");
                assert!(msg.contains(bad), "msg for `{bad}`: {msg}");
            }
            other => panic!("wrong variant for `{bad}`: {other:?}"),
        }
    }
}

#[test]
fn initiative_brief_rejects_unknown_top_level_frontmatter_key() {
    let yaml = "---\nname: solo\nfoo: bar\n---\n\nbody\n";
    let tmp = scaffold_initiative_brief(yaml);
    let err = InitiativeBrief::load(tmp.path()).expect_err("unknown top-level key");
    match err {
        Error::Config(msg) => assert!(msg.contains("foo"), "msg: {msg}"),
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn initiative_brief_rejects_unknown_input_key() {
    let yaml = "\
---
name: solo
inputs:
  - path: ./x
    kind: legacy-code
    extra: nope
---

body
";
    let tmp = scaffold_initiative_brief(yaml);
    let err = InitiativeBrief::load(tmp.path()).expect_err("unknown input key");
    match err {
        Error::Config(msg) => assert!(msg.contains("extra"), "msg: {msg}"),
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn initiative_brief_rejects_unknown_kind() {
    let yaml = "\
---
name: solo
inputs:
  - path: ./x
    kind: whatever
---

body
";
    let tmp = scaffold_initiative_brief(yaml);
    let err = InitiativeBrief::load(tmp.path()).expect_err("unknown kind");
    match err {
        Error::Config(msg) => {
            assert!(msg.contains("whatever"), "msg should mention bad value: {msg}");
            assert!(
                msg.contains("legacy-code")
                    || msg.contains("documentation")
                    || msg.contains("variant"),
                "msg should hint at closed enum: {msg}"
            );
        }
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn initiative_brief_rejects_missing_path() {
    let yaml = "\
---
name: solo
inputs:
  - kind: legacy-code
---

body
";
    let tmp = scaffold_initiative_brief(yaml);
    let err = InitiativeBrief::load(tmp.path()).expect_err("missing path");
    match err {
        Error::Config(msg) => assert!(msg.contains("path"), "msg: {msg}"),
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn initiative_brief_rejects_missing_kind() {
    let yaml = "\
---
name: solo
inputs:
  - path: ./x
---

body
";
    let tmp = scaffold_initiative_brief(yaml);
    let err = InitiativeBrief::load(tmp.path()).expect_err("missing kind");
    match err {
        Error::Config(msg) => assert!(msg.contains("kind"), "msg: {msg}"),
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn initiative_brief_rejects_empty_path_string() {
    let yaml = "\
---
name: solo
inputs:
  - path: \"\"
    kind: legacy-code
---

body
";
    let tmp = scaffold_initiative_brief(yaml);
    let err = InitiativeBrief::load(tmp.path()).expect_err("empty path");
    match err {
        Error::Config(msg) => {
            assert!(msg.contains("path"), "msg: {msg}");
            assert!(msg.contains("empty"), "msg: {msg}");
        }
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn initiative_brief_template_matches_golden() {
    let rendered = InitiativeBrief::template("traffic-modernisation");
    assert_eq!(rendered, TRAFFIC_TEMPLATE_GOLDEN);
}

#[test]
fn initiative_brief_template_title_cases_multi_word_name() {
    let rendered = InitiativeBrief::template("auth-token-refresh");
    assert!(
        rendered.contains("# Auth token refresh\n"),
        "expected title-cased heading, got:\n{rendered}"
    );
    assert!(
        rendered.contains("name: auth-token-refresh\n"),
        "frontmatter name must be the raw kebab form, got:\n{rendered}"
    );
}

#[test]
fn initiative_brief_rendered_template_round_trips() {
    let rendered = InitiativeBrief::template("my-initiative");
    let parsed = InitiativeBrief::parse_str(&rendered).expect("template parses");
    assert_eq!(parsed.frontmatter.name, "my-initiative");
    assert!(parsed.frontmatter.inputs.is_empty());
    assert!(parsed.body.contains("# My initiative"));
}

#[test]
fn initiative_brief_roundtrip_preserves_body() {
    // The closing `---\n` line itself is consumed by the delimiter
    // split; the body is *everything after* that line. Subsequent
    // `---` runs inside the body are preserved verbatim.
    let body = "\n# Title\n\nArbitrary prose\nwith multiple lines.\n\n---\n\nEven an embedded --- is fine once past the closing delimiter.\n";
    let raw = format!("---\nname: solo\n---\n{body}");
    let tmp = scaffold_initiative_brief(&raw);
    let brief = InitiativeBrief::load(tmp.path()).unwrap().unwrap();
    assert_eq!(brief.body, body);
}

#[test]
fn initiative_brief_path_helper_points_at_specify_dir() {
    let dir = Path::new("/tmp/some/project");
    assert_eq!(
        InitiativeBrief::path(dir),
        PathBuf::from("/tmp/some/project/.specify/initiative.md")
    );
}
