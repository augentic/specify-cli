//! Integration tests for `specify_domain::registry::registry` and
//! `specify_domain::registry::validate`.
//!
//! Lifted from `crates/capability/src/tests.rs` as part of RFC-13 chunk
//! 2.1 (extract platform-component artefacts out of the capability
//! crate). The tests cover `Registry::load`, `validate_shape`,
//! `validate_shape_hub`, URL classification, and the contract-roles
//! invariants (RFC-8 Layer 2 / RFC-12).

use std::path::{Path, PathBuf};

use specify_domain::registry::{ContractRoles, Registry, RegistryProject};
use specify_error::Error;
use tempfile::TempDir;

/// Scaffold `registry.yaml` (at the repo root) with `contents` and
/// return the containing project directory.
fn scaffold_registry(contents: &str) -> TempDir {
    let tmp = TempDir::new().unwrap();
    std::fs::write(Registry::path(tmp.path()), contents).unwrap();
    tmp
}

const CANONICAL_REGISTRY_YAML: &str = "\
version: 1
projects:
  - name: traffic
    url: .
    capability: omnia@v1
";

const MULTI_PROJECT_REGISTRY_YAML: &str = "\
version: 1
projects:
  - name: traffic
    url: .
    capability: omnia@v1
    description: Real-time traffic routing service
  - name: ingest
    url: git@github.com:augentic/ingest.git
    capability: omnia@v1
    description: Data ingestion pipeline
  - name: ops-runbook
    url: https://github.com/augentic/ops-runbook
    capability: omnia@v1
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
    assert_eq!(registry.projects[0].capability, "omnia@v1");
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
        Error::Diag { detail: msg, .. } => {
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
    capability: omnia@v1
    foo: bar
";
    let tmp = scaffold_registry(yaml);
    let err = Registry::load(tmp.path()).expect_err("unknown project key");
    match err {
        Error::Diag { detail: msg, .. } => {
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
        Error::Diag { detail: msg, .. } => {
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
        Error::Diag { detail: msg, .. } => assert!(msg.contains("version"), "msg: {msg}"),
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn registry_rejects_missing_name() {
    let yaml = "\
version: 1
projects:
  - url: .
    capability: omnia@v1
";
    let tmp = scaffold_registry(yaml);
    let err = Registry::load(tmp.path()).expect_err("missing name");
    assert!(matches!(err, Error::Diag { .. }), "got: {err:?}");
}

#[test]
fn registry_rejects_missing_url() {
    let yaml = "\
version: 1
projects:
  - name: traffic
    capability: omnia@v1
";
    let tmp = scaffold_registry(yaml);
    let err = Registry::load(tmp.path()).expect_err("missing url");
    assert!(matches!(err, Error::Diag { .. }), "got: {err:?}");
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
    assert!(matches!(err, Error::Diag { .. }), "got: {err:?}");
}

#[test]
fn registry_rejects_non_kebab_case_name() {
    for bad in ["TrafficSystem", "traffic_system", "traffic--system", "-traffic", "traffic-"] {
        let yaml = format!(
            "version: 1\nprojects:\n  - name: {bad}\n    url: .\n    capability: omnia@v1\n"
        );
        let tmp = scaffold_registry(&yaml);
        let err = Registry::load(tmp.path()).expect_err(&format!("bad name `{bad}`"));
        match err {
            Error::Diag { detail: msg, .. } => {
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
    capability: omnia@v1
";
    let tmp = scaffold_registry(yaml);
    let err = Registry::load(tmp.path()).expect_err("empty name");
    match err {
        Error::Diag { detail: msg, .. } => {
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
    capability: omnia@v1
";
    let tmp = scaffold_registry(yaml);
    let err = Registry::load(tmp.path()).expect_err("empty url");
    match err {
        Error::Diag { detail: msg, .. } => assert!(msg.contains("url"), "msg: {msg}"),
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
    capability: \"\"
";
    let tmp = scaffold_registry(yaml);
    let err = Registry::load(tmp.path()).expect_err("empty capability");
    match err {
        Error::Diag { detail: msg, .. } => assert!(msg.contains("capability"), "msg: {msg}"),
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
    capability: omnia@v1
  - name: traffic
    url: ../other
    capability: omnia@v1
";
    let tmp = scaffold_registry(yaml);
    let err = Registry::load(tmp.path()).expect_err("duplicate name");
    match err {
        Error::Diag { detail: msg, .. } => {
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
                capability: "omnia@v1".into(),
                description: Some("Real-time traffic routing".into()),
                contracts: None,
            },
            RegistryProject {
                name: "ingest".into(),
                url: "git@github.com:augentic/ingest.git".into(),
                capability: "omnia@v1".into(),
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
    capability: omnia@v1
    description: The alpha service
  - name: beta
    url: ../beta
    capability: omnia@v1
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
    capability: omnia@v1
    description: The alpha service
  - name: beta
    url: ../beta
    capability: omnia@v1
";
    let tmp = scaffold_registry(yaml);
    let err = Registry::load(tmp.path()).expect_err("missing description in multi-project");
    match err {
        Error::Diag { code, detail: msg } => {
            assert_eq!(code, "registry-description-missing-multi-repo", "msg: {msg}");
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
    capability: omnia@v1
    description: \"  \"
  - name: beta
    url: ../beta
    capability: omnia@v1
    description: The beta service
";
    let tmp = scaffold_registry(yaml);
    let err = Registry::load(tmp.path()).expect_err("whitespace-only description in multi-project");
    match err {
        Error::Diag { code, detail: msg } => {
            assert_eq!(code, "registry-description-missing-multi-repo", "msg: {msg}");
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
        capability: "omnia@v1".into(),
        description: Some("Real-time traffic routing".into()),
        contracts: None,
    };
    let yaml = serde_saphyr::to_string(&original).expect("serialize");
    let round_tripped: RegistryProject = serde_saphyr::from_str(&yaml).expect("re-parse");
    assert_eq!(round_tripped, original);
}

#[test]
fn registry_path_helper_points_at_repo_root() {
    let dir = Path::new("/tmp/some/project");
    assert_eq!(Registry::path(dir), PathBuf::from("/tmp/some/project/registry.yaml"));
}

// ---------- Registry URL validation (RFC-3a C28) ----------

fn registry_with_one_url(url: &str) -> Registry {
    Registry {
        version: 1,
        projects: vec![RegistryProject {
            name: "traffic".into(),
            url: url.into(),
            capability: "omnia@v1".into(),
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
            capability: "omnia@v1".into(),
            description: None,
            contracts: None,
        };
        assert_eq!(p.is_local(), symlink, "url={url:?}");
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
        Error::Diag { detail: msg, .. } => {
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
    assert!(matches!(err, Error::Diag { .. }), "got: {err:?}");
}

#[test]
fn registry_rejects_colon_without_scheme_or_git_at() {
    let err = registry_with_one_url("weird:path")
        .validate_shape()
        .expect_err("colon form must be rejected");
    match err {
        Error::Diag { detail: msg, .. } => assert!(msg.contains(':'), "msg: {msg}"),
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn registry_rejects_absolute_unix_path_as_url() {
    let err = registry_with_one_url("/absolute/path")
        .validate_shape()
        .expect_err("absolute path must be rejected");
    match err {
        Error::Diag { detail: msg, .. } => assert!(msg.contains("relative"), "msg: {msg}"),
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn registry_rejects_whitespace_only_url() {
    let err =
        registry_with_one_url("   ").validate_shape().expect_err("whitespace url must be rejected");
    match err {
        Error::Diag { detail: msg, .. } => assert!(msg.contains("whitespace"), "msg: {msg}"),
        other => panic!("wrong variant: {other:?}"),
    }
}

#[test]
fn registry_rejects_url_with_leading_whitespace() {
    let err = registry_with_one_url(" https://example.com/a")
        .validate_shape()
        .expect_err("leading space must be rejected");
    match err {
        Error::Diag { detail: msg, .. } => assert!(msg.contains("whitespace"), "msg: {msg}"),
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
                capability: "omnia@v1".into(),
                description: Some("Alpha service".into()),
                contracts: None,
            },
            RegistryProject {
                name: "beta".into(),
                url: "../beta".into(),
                capability: "omnia@v1".into(),
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
            capability: "omnia@v1".into(),
            description: None,
            contracts: None,
        }],
    };
    let err = reg.validate_shape_hub().expect_err("hub mode must reject url: .");
    match err {
        Error::Diag { code, detail: msg } => {
            assert_eq!(
                code, "hub-cannot-be-project",
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
                capability: "omnia@v1".into(),
                description: Some("Alpha service".into()),
                contracts: None,
            },
            RegistryProject {
                name: "self-as-project".into(),
                url: ".".into(),
                capability: "omnia@v1".into(),
                description: Some("Should be the hub, not an entry".into()),
                contracts: None,
            },
        ],
    };
    let err = reg.validate_shape_hub().expect_err("hub mode rejects `.` even alongside peers");
    match err {
        Error::Diag { code, detail: msg } => {
            assert_eq!(code, "hub-cannot-be-project", "msg: {msg}");
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
        Error::Diag { code, detail: msg } => {
            assert!(msg.contains("version"), "msg: {msg}");
            assert_ne!(
                code, "hub-cannot-be-project",
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
            capability: "omnia@v1".into(),
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
    capability: omnia@v1
    description: Real-time traffic routing service
    contracts:
      produces:
        - http/traffic-api.yaml
      consumes:
        - http/ingest-api.yaml
  - name: ingest
    url: git@github.com:augentic/ingest.git
    capability: omnia@v1
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
            capability: "omnia@v1".into(),
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
            capability: "omnia@v1".into(),
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
    capability: omnia@v1
    description: Alpha service
    contracts:
      produces:
        - http/shared-api.yaml
  - name: beta
    url: ../beta
    capability: omnia@v1
    description: Beta service
    contracts:
      produces:
        - http/shared-api.yaml
";
    let tmp = scaffold_registry(yaml);
    let err = Registry::load(tmp.path()).expect_err("single producer violation");
    match err {
        Error::Diag { detail: msg, .. } => {
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
    capability: omnia@v1
    contracts:
      imports:
        - http/external-api.yaml
";
    let tmp = scaffold_registry(yaml);
    let err = Registry::load(tmp.path()).expect_err("legacy imports field rejected");
    match err {
        Error::Diag { detail: msg, .. } => assert!(msg.contains("imports"), "msg: {msg}"),
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
    capability: omnia@v1
    contracts:
      produces:
        - /absolute/path.yaml
";
    let tmp = scaffold_registry(yaml);
    let err = Registry::load(tmp.path()).expect_err("absolute path rejected");
    match err {
        Error::Diag { detail: msg, .. } => {
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
    capability: omnia@v1
    contracts:
      consumes:
        - ../escape/path.yaml
";
    let tmp = scaffold_registry(yaml);
    let err = Registry::load(tmp.path()).expect_err(".. path rejected");
    match err {
        Error::Diag { detail: msg, .. } => {
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
    capability: omnia@v1
    contracts:
      produces:
        - http/my-api.yaml
      consumes:
        - http/my-api.yaml
";
    let tmp = scaffold_registry(yaml);
    let err = Registry::load(tmp.path()).expect_err("self-consistency violation");
    match err {
        Error::Diag { detail: msg, .. } => {
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
    capability: omnia@v1
    contracts:
      produces:
        - http/api.yaml
      bogus:
        - something
";
    let tmp = scaffold_registry(yaml);
    let err = Registry::load(tmp.path()).expect_err("unknown contract key");
    match err {
        Error::Diag { detail: msg, .. } => assert!(msg.contains("bogus"), "msg: {msg}"),
        other => panic!("wrong variant: {other:?}"),
    }
}
