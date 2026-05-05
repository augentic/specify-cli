//! Unit tests for `specify-capability`.
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
use crate::capability::{Capability, CapabilitySource, Phase};
use crate::initiative_brief::{InitiativeBrief, InputKind};
use crate::pipeline::PipelineView;

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

fn omnia_capability_path() -> PathBuf {
    repo_root().join("schemas").join("omnia").join("capability.yaml")
}

// ---------- Capability parsing ----------

#[test]
fn parses_omnia_capability_yaml_fields_and_entries() {
    let raw = std::fs::read_to_string(omnia_capability_path()).expect("omnia capability on disk");
    let schema: Capability = serde_saphyr::from_str(&raw).expect("omnia capability is valid YAML");

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
    let raw = std::fs::read_to_string(omnia_capability_path()).unwrap();
    let schema: Capability = serde_saphyr::from_str(&raw).unwrap();
    let results = schema.validate_structure();
    assert!(
        results.iter().all(|r| matches!(r, ValidationResult::Pass { .. })),
        "expected all passes, got: {results:?}"
    );
}

/// Phase 1.8 invariant: the canonical omnia `capability.yaml` carries
/// none of the dropped fields. RFC-13 §Capability manifest and
/// protocol froze the post-Phase-1 manifest at `name`, `version`,
/// `description`, and `pipeline { define, build, merge }` only. The
/// JSON Schema validator already enforces this for arbitrary inputs;
/// this test pins the bundled fixture itself so a future hand-edit of
/// `schemas/omnia/capability.yaml` cannot quietly reintroduce the
/// pre-RFC-13 fields.
#[test]
fn omnia_capability_yaml_has_no_dropped_fields() {
    let raw = std::fs::read_to_string(omnia_capability_path()).unwrap();

    for forbidden in ["domain:", "extends:"] {
        let starts_at_col_zero =
            raw.lines().any(|line| line.starts_with(forbidden));
        assert!(
            !starts_at_col_zero,
            "post-RFC-13 omnia capability must not carry top-level `{forbidden}`"
        );
    }

    let pipeline_plan_present = raw
        .lines()
        .map(str::trim_end)
        .any(|line| line == "  plan:" || line == "  plan: []");
    assert!(
        !pipeline_plan_present,
        "post-RFC-13 omnia capability must not declare `pipeline.plan` \
         (planning moves to the change surface in Phase 3)"
    );

    let parsed: Capability = serde_saphyr::from_str(&raw).expect("omnia capability parses");
    assert!(
        parsed.pipeline.plan.is_empty(),
        "parsed omnia pipeline.plan must be empty after Phase 1"
    );
}

#[test]
fn validate_structure_fails_when_define_phase_is_empty() {
    let schema = Capability {
        name: "broken".into(),
        version: 1,
        description: "empty define phase".into(),
        pipeline: crate::capability::Pipeline {
            plan: vec![],
            define: vec![],
            build: vec![crate::capability::PipelineEntry {
                id: "build".into(),
                brief: "briefs/build.md".into(),
            }],
            merge: vec![crate::capability::PipelineEntry {
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
    // `Error::Yaml` when surfaced through `Capability::resolve`, but
    // here we just exercise the parser directly and assert the Display
    // message.
    let yaml = "name: broken\nversion: 1\npipeline:\n  define: []\n  build: []\n  merge: []\n";
    let err = serde_saphyr::from_str::<Capability>(yaml).expect_err("missing description");
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
    let schema: Capability = serde_saphyr::from_str(yaml).expect("parses");
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

    // RFC-13 chunk 1.4: the JSON Schema now actively rejects
    // `pipeline.plan` (planning leaves the capability surface and moves
    // to `specify change`). The parser stays tolerant for one more
    // chunk so existing on-disk manifests load, but `validate_structure`
    // is the boundary that pins the post-RFC field set.
    let results = schema.validate_structure();
    let detail = results
        .iter()
        .find_map(|r| match r {
            ValidationResult::Fail { detail, .. } => Some(detail.as_str()),
            _ => None,
        })
        .expect("plan-bearing schema must fail validation");
    assert!(
        detail.contains("plan"),
        "rejection diagnostic must name the offending field, got: {detail}"
    );
}

#[test]
fn pipeline_without_plan_parses_unchanged() {
    let raw = std::fs::read_to_string(omnia_capability_path()).unwrap();
    let schema: Capability = serde_saphyr::from_str(&raw).unwrap();
    assert!(schema.pipeline.plan.is_empty());
    assert!(schema.plan_entries().is_empty());

    // Serializing back out must not introduce a `plan: []` key — we
    // skip-serialize empty plan vectors so round-trips of legacy
    // manifests are byte-stable for the plan field.
    let written = serde_saphyr::to_string(&schema).unwrap();
    assert!(
        !written.contains("plan:"),
        "expected no plan key in re-serialized omnia capability, got:\n{written}"
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
    let parent: Capability = serde_saphyr::from_str(parent_yaml).unwrap();
    let child: Capability = serde_saphyr::from_str(child_yaml).unwrap();
    let merged = Capability::merge(parent, child);

    let ids: Vec<&str> = merged.plan_entries().iter().map(|e| e.id.as_str()).collect();
    assert_eq!(ids, vec!["discovery", "propose", "record"]);
    let propose = merged.plan_entries().iter().find(|e| e.id == "propose").unwrap();
    assert_eq!(propose.brief, "briefs/plan/propose-v2.md");
}

/// JSON Schema body shipped with the crate. Read directly from disk so
/// the rejection tests below stay coupled to the on-disk file used by
/// `Capability::validate_structure`.
const CAPABILITY_JSON_SCHEMA: &str = include_str!("../../../schemas/capability.schema.json");

fn validate_raw(instance: &serde_json::Value) -> Vec<ValidationResult> {
    crate::capability::validate_against_embedded_schema(
        CAPABILITY_JSON_SCHEMA,
        "capability.valid",
        "capability manifest conforms to schemas/capability.schema.json",
        instance,
    )
}

fn fail_detail(results: &[ValidationResult], context: &str) -> String {
    results
        .iter()
        .find_map(|r| match r {
            ValidationResult::Fail { detail, .. } => Some(detail.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("expected failure for {context}, got: {results:?}"))
}

#[test]
fn json_schema_rejects_capability_domain_field() {
    let instance = serde_json::json!({
        "name": "broken",
        "version": 1,
        "description": "manifest still carrying legacy `domain`",
        "domain": "Tech stack: Rust",
        "pipeline": {
            "define": [{ "id": "proposal", "brief": "briefs/proposal.md" }],
            "build": [{ "id": "build", "brief": "briefs/build.md" }],
            "merge": [{ "id": "merge", "brief": "briefs/merge.md" }]
        }
    });
    let results = validate_raw(&instance);
    let detail = fail_detail(&results, "domain field");
    assert!(detail.contains("domain"), "diagnostic must name `domain`, got: {detail}");
}

#[test]
fn json_schema_rejects_capability_extends_field() {
    let instance = serde_json::json!({
        "name": "broken",
        "version": 1,
        "description": "manifest still carrying legacy `extends`",
        "extends": "https://example.com/parent.yaml",
        "pipeline": {
            "define": [{ "id": "proposal", "brief": "briefs/proposal.md" }],
            "build": [{ "id": "build", "brief": "briefs/build.md" }],
            "merge": [{ "id": "merge", "brief": "briefs/merge.md" }]
        }
    });
    let results = validate_raw(&instance);
    let detail = fail_detail(&results, "extends field");
    assert!(detail.contains("extends"), "diagnostic must name `extends`, got: {detail}");
}

#[test]
fn json_schema_rejects_pipeline_plan_block() {
    // RFC-13 chunk 1.4 tightens the JSON Schema to forbid
    // `pipeline.plan` outright — planning leaves the capability surface
    // and moves to the change surface. A manifest that still carries a
    // `plan` block must fail structure validation even when the rest of
    // the pipeline is well-formed.
    let schema = Capability {
        name: "broken".into(),
        version: 1,
        description: "plan still present".into(),
        pipeline: crate::capability::Pipeline {
            plan: vec![crate::capability::PipelineEntry {
                id: "discovery".into(),
                brief: "briefs/plan/discovery.md".into(),
            }],
            define: vec![crate::capability::PipelineEntry {
                id: "proposal".into(),
                brief: "briefs/proposal.md".into(),
            }],
            build: vec![crate::capability::PipelineEntry {
                id: "build".into(),
                brief: "briefs/build.md".into(),
            }],
            merge: vec![crate::capability::PipelineEntry {
                id: "merge".into(),
                brief: "briefs/merge.md".into(),
            }],
        },
    };
    let results = schema.validate_structure();
    let detail = results
        .iter()
        .find_map(|r| match r {
            ValidationResult::Fail { detail, .. } => Some(detail.as_str()),
            _ => None,
        })
        .expect("pipeline.plan must be rejected");
    assert!(
        detail.contains("plan"),
        "rejection diagnostic must name the offending field, got: {detail}"
    );
}

// ---------- Manifest composition ----------

#[test]
fn merge_overrides_by_id_and_appends_new_entries() {
    let parent_yaml = r"
name: parent
version: 1
description: parent capability
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
description: child capability
pipeline:
  define:
    - { id: specs,   brief: briefs/specs-v2.md }
    - { id: review,  brief: briefs/review.md }
  build:
    - { id: build, brief: briefs/build.md }
  merge:
    - { id: merge, brief: briefs/merge.md }
";

    let parent: Capability = serde_saphyr::from_str(parent_yaml).unwrap();
    let child: Capability = serde_saphyr::from_str(child_yaml).unwrap();
    let merged = Capability::merge(parent, child);

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
    let raw = std::fs::read_to_string(omnia_capability_path()).unwrap();
    let schema: Capability = serde_saphyr::from_str(&raw).unwrap();

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
    let raw = std::fs::read_to_string(omnia_capability_path()).unwrap();
    let schema: Capability = serde_saphyr::from_str(&raw).unwrap();
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
    assert!(matches!(view.schema.source, CapabilitySource::Local(_)));

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

/// Scaffold a minimal local capability at `<project>/schemas/<name>/`
/// with the given `schema.yaml` and brief contents. Each brief content
/// map entry is `(filename, contents)` written under `schemas/<name>/`.
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
    let err = Capability::resolve("https://example.com/schemas/nope", tmp.path())
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

    let resolved = Capability::resolve("demo", tmp.path()).unwrap();
    assert!(matches!(resolved.source, CapabilitySource::Cached(_)));
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

// ---------- Initiative brief (RFC-3a §"The Initiative Brief") ----------

/// Scaffold `initiative.md` (at the repo root) with `contents` and
/// return the containing project directory.
fn scaffold_initiative_brief(contents: &str) -> TempDir {
    let tmp = TempDir::new().unwrap();
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
     achieve. Plans reference this brief via `initiative.md`. -->
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
fn initiative_brief_path_helper_points_at_repo_root() {
    let dir = Path::new("/tmp/some/project");
    assert_eq!(InitiativeBrief::path(dir), PathBuf::from("/tmp/some/project/initiative.md"));
}
