use std::collections::{BTreeMap, HashSet};

use crate::registry::{Registry, RegistryProject};
use tempfile::tempdir;

use super::super::model::{Entry, Plan, Severity, Status};
use super::super::test_support::{RFC_EXAMPLE_YAML, change, plan_with_changes};

#[test]
fn clean_plan_validates() {
    let plan: Plan = serde_saphyr::from_str(RFC_EXAMPLE_YAML).expect("parse rfc fixture");
    let results = plan.validate(None, None);
    assert!(
        results.is_empty(),
        "expected a clean RFC fixture to validate with no findings, got: {results:#?}"
    );
}

#[test]
fn duplicate_name_error() {
    let plan = plan_with_changes(vec![change("foo", Status::Done), change("foo", Status::Pending)]);
    let results = plan.validate(None, None);
    let dupes: Vec<_> = results.iter().filter(|r| r.code == "duplicate-name").collect();
    assert_eq!(dupes.len(), 1, "expected one duplicate-name result, got {results:#?}");
    assert_eq!(dupes[0].level, Severity::Error);
    assert_eq!(dupes[0].entry.as_deref(), Some("foo"));
}

#[test]
fn cycle_error() {
    let mut a = change("a", Status::Pending);
    a.depends_on = vec!["c".into()];
    let mut b = change("b", Status::Pending);
    b.depends_on = vec!["a".into()];
    let mut c = change("c", Status::Pending);
    c.depends_on = vec!["b".into()];
    let plan = plan_with_changes(vec![a, b, c]);
    let results = plan.validate(None, None);
    let cycles: Vec<_> = results.iter().filter(|r| r.code == "dependency-cycle").collect();
    assert!(!cycles.is_empty(), "expected at least one dependency-cycle, got {results:#?}");
    let msg = &cycles[0].message;
    assert!(msg.contains('a'), "cycle message should name a: {msg}");
    assert!(msg.contains('b'), "cycle message should name b: {msg}");
    assert!(msg.contains('c'), "cycle message should name c: {msg}");
}

#[test]
fn self_cycle_error() {
    let mut a = change("a", Status::Pending);
    a.depends_on = vec!["a".into()];
    let plan = plan_with_changes(vec![a]);
    let results = plan.validate(None, None);
    assert!(
        results.iter().any(|r| r.code == "dependency-cycle"),
        "expected a dependency-cycle result for self-edge, got: {results:#?}"
    );
}

#[test]
fn unknown_depends_on_error() {
    let mut a = change("a", Status::Pending);
    a.depends_on = vec!["bogus".into()];
    let plan = plan_with_changes(vec![a]);
    let results = plan.validate(None, None);
    let hits: Vec<_> = results.iter().filter(|r| r.code == "unknown-depends-on").collect();
    assert_eq!(hits.len(), 1, "expected one unknown-depends-on, got {results:#?}");
    assert_eq!(hits[0].entry.as_deref(), Some("a"));
    assert!(hits[0].message.contains("bogus"));
}

#[test]
fn unknown_source_error() {
    let mut a = change("a", Status::Pending);
    a.sources = vec!["monolith".into()];
    let plan = plan_with_changes(vec![a]);
    let results = plan.validate(None, None);
    let hits: Vec<_> = results.iter().filter(|r| r.code == "unknown-source").collect();
    assert_eq!(hits.len(), 1, "expected one unknown-source, got {results:#?}");
    assert_eq!(hits[0].entry.as_deref(), Some("a"));
    assert!(hits[0].message.contains("monolith"));
}

#[test]
fn multiple_in_progress_error() {
    let plan =
        plan_with_changes(vec![change("a", Status::InProgress), change("b", Status::InProgress)]);
    let results = plan.validate(None, None);
    let hits: Vec<_> = results.iter().filter(|r| r.code == "multiple-in-progress").collect();
    assert_eq!(hits.len(), 2, "expected one result per offender, got {results:#?}");
    let names: HashSet<&str> = hits.iter().filter_map(|r| r.entry.as_deref()).collect();
    assert!(names.contains("a") && names.contains("b"), "names = {names:?}");
}

#[test]
fn single_in_progress_is_fine() {
    let plan =
        plan_with_changes(vec![change("a", Status::InProgress), change("b", Status::Pending)]);
    let results = plan.validate(None, None);
    assert!(
        !results.iter().any(|r| r.code == "multiple-in-progress"),
        "single in-progress entry should not trip multiple-in-progress: {results:#?}"
    );
}

#[test]
fn orphan_dir_warning() {
    let tmp = tempdir().expect("tempdir");
    std::fs::create_dir(tmp.path().join("stale-slice")).expect("mkdir");
    let plan = plan_with_changes(vec![change("other", Status::Pending)]);
    let results = plan.validate(Some(tmp.path()), None);
    let hits: Vec<_> = results.iter().filter(|r| r.code == "orphan-slice-dir").collect();
    assert_eq!(hits.len(), 1, "expected one orphan-slice-dir, got {results:#?}");
    assert_eq!(hits[0].level, Severity::Warning);
    assert_eq!(hits[0].entry.as_deref(), Some("stale-slice"));
}

#[test]
fn missing_dir_for_in_progress_warning() {
    let tmp = tempdir().expect("tempdir");
    let plan = plan_with_changes(vec![change("alpha", Status::InProgress)]);
    let results = plan.validate(Some(tmp.path()), None);
    let hits: Vec<_> =
        results.iter().filter(|r| r.code == "missing-slice-dir-for-in-progress").collect();
    assert_eq!(hits.len(), 1, "expected one missing-dir warning, got {results:#?}");
    assert_eq!(hits[0].level, Severity::Warning);
    assert_eq!(hits[0].entry.as_deref(), Some("alpha"));
}

#[test]
fn present_dir_for_in_progress_silent() {
    let tmp = tempdir().expect("tempdir");
    std::fs::create_dir(tmp.path().join("alpha")).expect("mkdir alpha");
    let plan = plan_with_changes(vec![change("alpha", Status::InProgress)]);
    let results = plan.validate(Some(tmp.path()), None);
    assert!(
        !results.iter().any(|r| r.code.ends_with("-slice-dir")
            || r.code == "orphan-slice-dir"
            || r.code == "missing-slice-dir-for-in-progress"),
        "no directory warnings expected, got: {results:#?}"
    );
}

#[test]
fn no_slices_dir_skips_consistency() {
    let plan = plan_with_changes(vec![change("alpha", Status::InProgress)]);
    let results = plan.validate(None, None);
    assert!(
        !results
            .iter()
            .any(|r| r.code == "orphan-slice-dir" || r.code == "missing-slice-dir-for-in-progress"),
        "passing None for slices_dir must skip directory consistency checks: {results:#?}"
    );
}

#[test]
fn no_short_circuit() {
    let mut a = change("foo", Status::Pending);
    a.depends_on = vec!["missing".into()];
    a.sources = vec!["ghost-source".into()];
    let b = change("foo", Status::Pending);
    let plan = plan_with_changes(vec![a, b]);
    let results = plan.validate(None, None);

    let codes: HashSet<&'static str> = results.iter().map(|r| r.code).collect();
    for expected in ["duplicate-name", "unknown-depends-on", "unknown-source"] {
        assert!(
            codes.contains(expected),
            "expected code {expected} in {codes:?} — validate must not short-circuit"
        );
    }
}

#[test]
fn project_not_in_registry() {
    let plan = Plan {
        name: "test".to_string(),
        sources: BTreeMap::new(),
        entries: vec![Entry {
            name: "a".to_string(),
            project: Some("nonexistent".to_string()),
            capability: None,
            status: Status::Pending,
            depends_on: vec![],
            sources: vec![],
            context: vec![],
            description: None,
            status_reason: None,
        }],
    };
    let registry = Registry {
        version: 1,
        projects: vec![RegistryProject {
            name: "real-project".to_string(),
            url: ".".to_string(),
            capability: "omnia@v1".to_string(),
            description: None,
            contracts: None,
        }],
    };
    let results = plan.validate(None, Some(&registry));
    assert!(results.iter().any(|r| r.code == "project-not-in-registry"));
}

#[test]
fn project_missing_multi_repo() {
    let plan = Plan {
        name: "test".to_string(),
        sources: BTreeMap::new(),
        entries: vec![Entry {
            name: "a".to_string(),
            project: None,
            capability: None,
            status: Status::Pending,
            depends_on: vec![],
            sources: vec![],
            context: vec![],
            description: None,
            status_reason: None,
        }],
    };
    let registry = Registry {
        version: 1,
        projects: vec![
            RegistryProject {
                name: "alpha".to_string(),
                url: ".".to_string(),
                capability: "omnia@v1".to_string(),
                description: Some("Alpha project".to_string()),
                contracts: None,
            },
            RegistryProject {
                name: "beta".to_string(),
                url: "git@github.com:org/beta.git".to_string(),
                capability: "omnia@v1".to_string(),
                description: Some("Beta project".to_string()),
                contracts: None,
            },
        ],
    };
    let results = plan.validate(None, Some(&registry));
    assert!(results.iter().any(|r| r.code == "project-missing-multi-repo"));
}

#[test]
fn capability_only_entry_valid_multi_repo() {
    let plan = Plan {
        name: "test".to_string(),
        sources: BTreeMap::new(),
        entries: vec![Entry {
            name: "contracts".to_string(),
            project: None,
            capability: Some("contracts@v1".into()),
            status: Status::Pending,
            depends_on: vec![],
            sources: vec![],
            context: vec![],
            description: None,
            status_reason: None,
        }],
    };
    let registry = Registry {
        version: 1,
        projects: vec![
            RegistryProject {
                name: "alpha".to_string(),
                url: ".".to_string(),
                capability: "omnia@v1".to_string(),
                description: Some("Alpha project".to_string()),
                contracts: None,
            },
            RegistryProject {
                name: "beta".to_string(),
                url: "git@github.com:org/beta.git".to_string(),
                capability: "omnia@v1".to_string(),
                description: Some("Beta project".to_string()),
                contracts: None,
            },
        ],
    };
    let results = plan.validate(None, Some(&registry));
    assert!(
        !results.iter().any(|r| r.code == "project-missing-multi-repo"),
        "schema-only coordinator entries must remain valid in multi-repo plans: {results:#?}"
    );
}

#[test]
fn project_valid_single_repo() {
    let plan = Plan {
        name: "test".to_string(),
        sources: BTreeMap::new(),
        entries: vec![Entry {
            name: "a".to_string(),
            project: None,
            capability: Some("contracts@v1".into()),
            status: Status::Pending,
            depends_on: vec![],
            sources: vec![],
            context: vec![],
            description: None,
            status_reason: None,
        }],
    };
    let registry = Registry {
        version: 1,
        projects: vec![RegistryProject {
            name: "solo".to_string(),
            url: ".".to_string(),
            capability: "omnia@v1".to_string(),
            description: None,
            contracts: None,
        }],
    };
    let results = plan.validate(None, Some(&registry));
    assert!(!results.iter().any(|r| r.code == "project-missing-multi-repo"));
    assert!(!results.iter().any(|r| r.code == "project-not-in-registry"));
}

#[test]
fn project_matches_registry() {
    let plan = Plan {
        name: "test".to_string(),
        sources: BTreeMap::new(),
        entries: vec![Entry {
            name: "a".to_string(),
            project: Some("alpha".to_string()),
            capability: None,
            status: Status::Pending,
            depends_on: vec![],
            sources: vec![],
            context: vec![],
            description: None,
            status_reason: None,
        }],
    };
    let registry = Registry {
        version: 1,
        projects: vec![
            RegistryProject {
                name: "alpha".to_string(),
                url: ".".to_string(),
                capability: "omnia@v1".to_string(),
                description: Some("Alpha".to_string()),
                contracts: None,
            },
            RegistryProject {
                name: "beta".to_string(),
                url: "git@github.com:org/beta.git".to_string(),
                capability: "omnia@v1".to_string(),
                description: Some("Beta".to_string()),
                contracts: None,
            },
        ],
    };
    let results = plan.validate(None, Some(&registry));
    assert!(!results.iter().any(|r| r.level == Severity::Error));
}

#[test]
fn neither_project_nor_capability_error() {
    let plan = Plan {
        name: "test".to_string(),
        sources: BTreeMap::new(),
        entries: vec![Entry {
            name: "orphan".to_string(),
            project: None,
            capability: None,
            status: Status::Pending,
            depends_on: vec![],
            sources: vec![],
            context: vec![],
            description: None,
            status_reason: None,
        }],
    };
    let results = plan.validate(None, None);
    assert!(
        results
            .iter()
            .any(|r| r.code == "plan.entry-needs-project-or-capability"
                && r.level == Severity::Error),
        "expected entry-needs-project-or-capability error, got: {results:#?}"
    );
}

#[test]
fn capability_only_passes() {
    let plan = Plan {
        name: "test".to_string(),
        sources: BTreeMap::new(),
        entries: vec![Entry {
            name: "contracts".to_string(),
            project: None,
            capability: Some("contracts@v1".into()),
            status: Status::Pending,
            depends_on: vec![],
            sources: vec![],
            context: vec![],
            description: None,
            status_reason: None,
        }],
    };
    let results = plan.validate(None, None);
    assert!(
        !results.iter().any(|r| r.code == "plan.entry-needs-project-or-capability"),
        "capability-only entry must not trigger project-or-capability error"
    );
}

#[test]
fn project_and_capability_passes() {
    let plan = Plan {
        name: "test".to_string(),
        sources: BTreeMap::new(),
        entries: vec![Entry {
            name: "impl".to_string(),
            project: Some("auth-service".into()),
            capability: Some("omnia@v1".into()),
            status: Status::Pending,
            depends_on: vec![],
            sources: vec![],
            context: vec![],
            description: None,
            status_reason: None,
        }],
    };
    let results = plan.validate(None, None);
    assert!(
        !results.iter().any(|r| r.code == "plan.entry-needs-project-or-capability"),
        "entry with both project and capability must pass"
    );
}

#[test]
fn context_rejects_dotdot() {
    let mut entry = change("foo", Status::Pending);
    entry.context = vec!["../etc/passwd".into()];
    let plan = plan_with_changes(vec![entry]);
    let errors: Vec<_> = plan
        .validate(None, None)
        .into_iter()
        .filter(|r| r.code == "plan.context-path-invalid")
        .collect();
    assert_eq!(errors.len(), 1, "expected exactly one context-path-invalid error");
    assert!(errors[0].message.contains(".."), "message should mention '..'");
}

#[test]
fn context_rejects_absolute() {
    let mut entry = change("foo", Status::Pending);
    entry.context = vec!["/absolute/path".into()];
    let plan = plan_with_changes(vec![entry]);
    let errors: Vec<_> = plan
        .validate(None, None)
        .into_iter()
        .filter(|r| r.code == "plan.context-path-invalid")
        .collect();
    assert_eq!(errors.len(), 1, "expected exactly one context-path-invalid error");
    assert!(errors[0].message.contains("/absolute/path"));
}

#[test]
fn context_accepts_valid() {
    let mut entry = change("foo", Status::Pending);
    entry.context =
        vec!["contracts/http/user-api.yaml".into(), "specs/user-registration/spec.md".into()];
    let plan = plan_with_changes(vec![entry]);
    assert!(
        !plan.validate(None, None).into_iter().any(|r| r.code == "plan.context-path-invalid"),
        "valid relative paths must not produce errors"
    );
}
