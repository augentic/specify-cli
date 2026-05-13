use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use tempfile::tempdir;

use super::*;
use crate::change::plan::core::{Entry, Plan, Status};
use crate::registry::{Registry, RegistryProject};

fn change(name: &str, status: Status) -> Entry {
    Entry {
        name: name.into(),
        project: Some("default".into()),
        capability: None,
        status,
        depends_on: vec![],
        sources: vec![],
        context: vec![],
        description: None,
        status_reason: None,
    }
}

fn change_with_deps(name: &str, status: Status, deps: &[&str]) -> Entry {
    let mut e = change(name, status);
    e.depends_on = deps.iter().map(|s| (*s).to_string()).collect();
    e
}

fn plan_with(changes: Vec<Entry>) -> Plan {
    Plan {
        name: "test".into(),
        sources: BTreeMap::new(),
        entries: changes,
    }
}

fn plan_with_sources(sources: Vec<(&str, &str)>, changes: Vec<Entry>) -> Plan {
    let mut map = BTreeMap::new();
    for (k, v) in sources {
        map.insert(k.to_string(), v.to_string());
    }
    Plan {
        name: "test".into(),
        sources: map,
        entries: changes,
    }
}

// ------- 1. Cycle detection ----------------------------------------

#[test]
fn doctor_cycle_two_node() {
    let plan = plan_with(vec![
        change_with_deps("a", Status::Pending, &["b"]),
        change_with_deps("b", Status::Pending, &["a"]),
    ]);
    let hits: Vec<_> =
        doctor(&plan, None, None, None).into_iter().filter(|d| d.code == CYCLE).collect();
    assert_eq!(hits.len(), 1, "expected one cycle, got {hits:#?}");
    match hits[0].data.as_ref().unwrap() {
        DiagnosticPayload::Cycle { cycle } => {
            assert_eq!(cycle, &vec!["a".to_string(), "b".to_string(), "a".to_string()]);
        }
        other => panic!("wrong payload: {other:?}"),
    }
}

#[test]
fn doctor_cycle_three_node() {
    let plan = plan_with(vec![
        change_with_deps("a", Status::Pending, &["c"]),
        change_with_deps("b", Status::Pending, &["a"]),
        change_with_deps("c", Status::Pending, &["b"]),
    ]);
    let hits: Vec<_> =
        doctor(&plan, None, None, None).into_iter().filter(|d| d.code == CYCLE).collect();
    assert_eq!(hits.len(), 1, "single SCC, single diagnostic");
    match hits[0].data.as_ref().unwrap() {
        DiagnosticPayload::Cycle { cycle } => {
            assert_eq!(
                cycle,
                &vec!["a".to_string(), "b".to_string(), "c".to_string(), "a".to_string()]
            );
        }
        other => panic!("wrong payload: {other:?}"),
    }
}

#[test]
fn doctor_cycle_two_disjoint() {
    let plan = plan_with(vec![
        change_with_deps("a", Status::Pending, &["b"]),
        change_with_deps("b", Status::Pending, &["a"]),
        change_with_deps("c", Status::Pending, &["d"]),
        change_with_deps("d", Status::Pending, &["c"]),
    ]);
    let count = doctor(&plan, None, None, None).into_iter().filter(|d| d.code == CYCLE).count();
    assert_eq!(count, 2, "expected two distinct cycles");
}

#[test]
fn doctor_cycle_self_loop() {
    let plan = plan_with(vec![change_with_deps("a", Status::Pending, &["a"])]);
    let hits: Vec<_> =
        doctor(&plan, None, None, None).into_iter().filter(|d| d.code == CYCLE).collect();
    assert_eq!(hits.len(), 1);
    match hits[0].data.as_ref().unwrap() {
        DiagnosticPayload::Cycle { cycle } => {
            assert_eq!(cycle, &vec!["a".to_string(), "a".to_string()]);
        }
        other => panic!("wrong payload: {other:?}"),
    }
}

#[test]
fn doctor_no_cycle_quiet() {
    let plan =
        plan_with(vec![change("a", Status::Done), change_with_deps("b", Status::Pending, &["a"])]);
    let hits: Vec<_> =
        doctor(&plan, None, None, None).into_iter().filter(|d| d.code == CYCLE).collect();
    assert!(hits.is_empty(), "no cycle expected, got {hits:#?}");
}

// ------- 2. Orphan source keys -------------------------------------

#[test]
fn doctor_orphan_source_zero() {
    let mut e = change("a", Status::Pending);
    e.sources = vec!["monolith".into()];
    let plan = plan_with_sources(vec![("monolith", "/path")], vec![e]);
    let any_orphan = doctor(&plan, None, None, None).into_iter().any(|d| d.code == ORPHAN_SOURCE);
    assert!(!any_orphan);
}

#[test]
fn doctor_orphan_source_one() {
    let plan = plan_with_sources(
        vec![("monolith", "/path"), ("orphan", "/elsewhere")],
        vec![{
            let mut e = change("a", Status::Pending);
            e.sources = vec!["monolith".into()];
            e
        }],
    );
    let hits: Vec<_> =
        doctor(&plan, None, None, None).into_iter().filter(|d| d.code == ORPHAN_SOURCE).collect();
    assert_eq!(hits.len(), 1);
    match hits[0].data.as_ref().unwrap() {
        DiagnosticPayload::OrphanSource { key } => assert_eq!(key, "orphan"),
        other => panic!("wrong payload: {other:?}"),
    }
    assert_eq!(hits[0].severity, Severity::Warning);
}

#[test]
fn doctor_orphan_source_multiple_sorted() {
    let plan = plan_with_sources(
        vec![("alpha", "/a"), ("beta", "/b"), ("gamma", "/g"), ("monolith", "/m")],
        vec![{
            let mut e = change("a", Status::Pending);
            e.sources = vec!["monolith".into()];
            e
        }],
    );
    let hits: Vec<_> =
        doctor(&plan, None, None, None).into_iter().filter(|d| d.code == ORPHAN_SOURCE).collect();
    let keys: Vec<&str> = hits
        .iter()
        .map(|d| match d.data.as_ref().unwrap() {
            DiagnosticPayload::OrphanSource { key } => key.as_str(),
            _ => panic!("wrong payload"),
        })
        .collect();
    assert_eq!(keys, vec!["alpha", "beta", "gamma"]);
}

#[test]
fn doctor_orphan_source_mixed_references() {
    let plan = plan_with_sources(
        vec![("monolith", "/m"), ("orders", "/o"), ("ghost", "/g")],
        vec![
            {
                let mut e = change("a", Status::Pending);
                e.sources = vec!["monolith".into(), "orders".into()];
                e
            },
            {
                let mut e = change("b", Status::Done);
                e.sources = vec!["orders".into()];
                e
            },
        ],
    );
    let count =
        doctor(&plan, None, None, None).into_iter().filter(|d| d.code == ORPHAN_SOURCE).count();
    assert_eq!(count, 1, "only `ghost` should orphan");
}

// ------- 3. Unreachable entries ------------------------------------

#[test]
fn doctor_unreachable_single_failed_predecessor() {
    let plan = plan_with(vec![
        change("a", Status::Failed),
        change_with_deps("b", Status::Pending, &["a"]),
    ]);
    let hits: Vec<_> =
        doctor(&plan, None, None, None).into_iter().filter(|d| d.code == UNREACHABLE).collect();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].entry.as_deref(), Some("b"));
    match hits[0].data.as_ref().unwrap() {
        DiagnosticPayload::UnreachableEntry { entry, blocking } => {
            assert_eq!(entry, "b");
            assert_eq!(blocking.len(), 1);
            assert_eq!(blocking[0].name, "a");
            assert_eq!(blocking[0].status, "failed");
        }
        other => panic!("wrong payload: {other:?}"),
    }
}

#[test]
fn doctor_unreachable_transitive_failure() {
    let plan = plan_with(vec![
        change("a", Status::Failed),
        change_with_deps("b", Status::Pending, &["a"]),
        change_with_deps("c", Status::Pending, &["b"]),
    ]);
    let hits: Vec<_> =
        doctor(&plan, None, None, None).into_iter().filter(|d| d.code == UNREACHABLE).collect();
    let names: Vec<&str> = hits.iter().filter_map(|d| d.entry.as_deref()).collect();
    assert_eq!(names, vec!["b", "c"], "both b and c are unreachable, sorted");
    let c = hits.iter().find(|d| d.entry.as_deref() == Some("c")).unwrap();
    match c.data.as_ref().unwrap() {
        DiagnosticPayload::UnreachableEntry { blocking, .. } => {
            assert_eq!(blocking.len(), 1);
            assert_eq!(blocking[0].name, "b");
            assert_eq!(blocking[0].status, "pending");
        }
        other => panic!("wrong payload: {other:?}"),
    }
}

#[test]
fn doctor_unreachable_mixed_terminal_predecessors() {
    let plan = plan_with(vec![
        change("a", Status::Failed),
        change("b", Status::Skipped),
        change_with_deps("c", Status::Pending, &["a", "b"]),
    ]);
    let hits: Vec<_> =
        doctor(&plan, None, None, None).into_iter().filter(|d| d.code == UNREACHABLE).collect();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].entry.as_deref(), Some("c"));
    match hits[0].data.as_ref().unwrap() {
        DiagnosticPayload::UnreachableEntry { blocking, .. } => {
            let mut names: Vec<&str> = blocking.iter().map(|b| b.name.as_str()).collect();
            names.sort_unstable();
            assert_eq!(names, vec!["a", "b"]);
            let mut statuses: Vec<&str> = blocking.iter().map(|b| b.status.as_str()).collect();
            statuses.sort_unstable();
            assert_eq!(statuses, vec!["failed", "skipped"]);
        }
        other => panic!("wrong payload: {other:?}"),
    }
}

#[test]
fn doctor_unreachable_skips_cycle_members() {
    // a-b cycle plus c-failed -> d-pending. Only d should be reported as
    // unreachable; a/b show up under cycle-in-depends-on.
    let plan = plan_with(vec![
        change_with_deps("a", Status::Pending, &["b"]),
        change_with_deps("b", Status::Pending, &["a"]),
        change("c", Status::Failed),
        change_with_deps("d", Status::Pending, &["c"]),
    ]);
    let unreach: Vec<_> =
        doctor(&plan, None, None, None).into_iter().filter(|d| d.code == UNREACHABLE).collect();
    let names: Vec<&str> = unreach.iter().filter_map(|d| d.entry.as_deref()).collect();
    assert_eq!(names, vec!["d"], "cycle members must not double-report");
}

#[test]
fn doctor_unreachable_quiet_on_healthy_plan() {
    let plan =
        plan_with(vec![change("a", Status::Done), change_with_deps("b", Status::Pending, &["a"])]);
    let hits: Vec<_> =
        doctor(&plan, None, None, None).into_iter().filter(|d| d.code == UNREACHABLE).collect();
    assert!(hits.is_empty(), "no unreachable expected, got {hits:#?}");
}

// ------- 4. Stale workspace clones --------------------------------

fn registry_with(projects: Vec<RegistryProject>) -> Registry {
    Registry { version: 1, projects }
}

fn rp(name: &str, url: &str, schema: &str, description: &str) -> RegistryProject {
    RegistryProject {
        name: name.into(),
        url: url.into(),
        capability: schema.into(),
        description: Some(description.into()),
        contracts: None,
    }
}

fn run_git(cwd: &Path, args: &[&str]) {
    let output = Command::new("git").arg("-C").arg(cwd).args(args).output().unwrap();
    assert!(
        output.status.success(),
        "git -C {} {} failed\nstdout:\n{}\nstderr:\n{}",
        cwd.display(),
        args.join(" "),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Set up a project root with a `.specify/workspace/<name>/` slot
/// wired as a git clone.
fn make_clone_slot(root: &Path, name: &str, origin: Option<&str>) -> std::path::PathBuf {
    let slot = root.join(".specify").join("workspace").join(name);
    std::fs::create_dir_all(&slot).unwrap();
    run_git(&slot, &["init"]);
    if let Some(origin) = origin {
        run_git(&slot, &["remote", "add", "origin", origin]);
    }
    slot
}

#[cfg(unix)]
fn symlink_dir(target: &Path, link: &Path) {
    std::os::unix::fs::symlink(target, link).unwrap();
}

#[cfg(windows)]
fn symlink_dir(target: &Path, link: &Path) {
    std::os::windows::fs::symlink_dir(target, link).unwrap();
}

#[test]
fn doctor_stale_clone_reports_missing_origin_without_sync_stamp_warning() {
    let tmp = tempdir().unwrap();
    let _slot = make_clone_slot(tmp.path(), "alpha", None);
    let registry = registry_with(vec![rp(
        "alpha",
        "git@github.com:org/alpha.git",
        "omnia@v1",
        "alpha service",
    )]);
    let plan = plan_with(vec![]);
    let hits: Vec<_> = doctor(&plan, None, Some(&registry), Some(tmp.path()))
        .into_iter()
        .filter(|d| d.code == STALE_CLONE)
        .collect();
    assert_eq!(hits.len(), 1, "expected single stale-clone, got {hits:#?}");
    match hits[0].data.as_ref().unwrap() {
        DiagnosticPayload::StaleClone {
            project,
            reason,
            expected,
            observed,
        } => {
            assert_eq!(project, "alpha");
            assert_eq!(*reason, StaleReason::SlotMismatch);
            assert_eq!(expected.as_ref().unwrap().slot_kind.as_deref(), Some("git-clone"));
            assert_eq!(observed.as_ref().unwrap().slot_kind.as_deref(), Some("git-clone"));
            assert!(observed.as_ref().unwrap().url.is_none());
        }
        other => panic!("wrong payload: {other:?}"),
    }
    assert!(
        hits[0].message.contains("has no origin remote"),
        "missing origin should be reported via sync slot rules: {:?}",
        hits[0].message
    );
}

#[test]
fn doctor_stale_clone_signature_changed() {
    let tmp = tempdir().unwrap();
    make_clone_slot(tmp.path(), "alpha", Some("git@github.com:old/alpha.git"));
    let registry = registry_with(vec![rp(
        "alpha",
        "git@github.com:org/alpha.git",
        "omnia@v1",
        "alpha service",
    )]);
    let plan = plan_with(vec![]);
    let hits: Vec<_> = doctor(&plan, None, Some(&registry), Some(tmp.path()))
        .into_iter()
        .filter(|d| d.code == STALE_CLONE)
        .collect();
    assert_eq!(hits.len(), 1);
    match hits[0].data.as_ref().unwrap() {
        DiagnosticPayload::StaleClone {
            reason,
            expected,
            observed,
            ..
        } => {
            assert_eq!(*reason, StaleReason::SignatureChanged);
            assert_eq!(
                expected.as_ref().unwrap().url.as_deref(),
                Some("git@github.com:org/alpha.git")
            );
            assert_eq!(
                observed.as_ref().unwrap().url.as_deref(),
                Some("git@github.com:old/alpha.git")
            );
        }
        other => panic!("wrong payload: {other:?}"),
    }
}

#[test]
fn doctor_stale_clone_signature_current() {
    let tmp = tempdir().unwrap();
    make_clone_slot(tmp.path(), "alpha", Some("git@github.com:org/alpha.git"));
    let registry = registry_with(vec![rp(
        "alpha",
        "git@github.com:org/alpha.git",
        "omnia@v1",
        "alpha service",
    )]);
    let plan = plan_with(vec![]);
    let hits: Vec<_> = doctor(&plan, None, Some(&registry), Some(tmp.path()))
        .into_iter()
        .filter(|d| d.code == STALE_CLONE)
        .collect();
    assert!(hits.is_empty(), "current signature must not warn, got {hits:#?}");
}

#[test]
fn doctor_stale_clone_diagnoses_wrong_symlink_target() {
    let tmp = tempdir().unwrap();
    let peer = tmp.path().join("peer");
    let other = tmp.path().join("other");
    std::fs::create_dir_all(&peer).unwrap();
    std::fs::create_dir_all(&other).unwrap();
    let workspace = tmp.path().join(".specify").join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    symlink_dir(&other, &workspace.join("peer"));
    let registry = registry_with(vec![rp("peer", "./peer", "omnia@v1", "peer service")]);
    let plan = plan_with(vec![]);
    let hits: Vec<_> = doctor(&plan, None, Some(&registry), Some(tmp.path()))
        .into_iter()
        .filter(|d| d.code == STALE_CLONE)
        .collect();
    assert_eq!(hits.len(), 1, "wrong symlink target must surface stale slot");
    match hits[0].data.as_ref().unwrap() {
        DiagnosticPayload::StaleClone {
            reason,
            expected,
            observed,
            ..
        } => {
            assert_eq!(*reason, StaleReason::SlotMismatch);
            assert_eq!(expected.as_ref().unwrap().slot_kind.as_deref(), Some("symlink"));
            assert_eq!(observed.as_ref().unwrap().slot_kind.as_deref(), Some("symlink"));
            assert!(
                observed.as_ref().unwrap().target.as_ref().unwrap().contains("other"),
                "observed target should name the wrong symlink target"
            );
        }
        other => panic!("wrong payload: {other:?}"),
    }
}

#[test]
fn doctor_stale_clone_ignores_missing_symlink_slots() {
    let tmp = tempdir().unwrap();
    let registry = registry_with(vec![rp("self", ".", "omnia@v1", "self service")]);
    let plan = plan_with(vec![]);
    let any_stale = doctor(&plan, None, Some(&registry), Some(tmp.path()))
        .into_iter()
        .any(|d| d.code == STALE_CLONE);
    assert!(!any_stale, "missing slots are left to workspace sync");
}

// ------- Combined / negative cases --------------------------------

#[test]
fn doctor_healthy_plan_emits_zero_doctor_diagnostics() {
    let plan = plan_with_sources(
        vec![("monolith", "/m")],
        vec![
            {
                let mut e = change("a", Status::Done);
                e.sources = vec!["monolith".into()];
                e
            },
            {
                let mut e = change_with_deps("b", Status::Pending, &["a"]);
                e.sources = vec!["monolith".into()];
                e
            },
        ],
    );
    let diagnostics = doctor(&plan, None, None, None);
    for code in [CYCLE, ORPHAN_SOURCE, STALE_CLONE, UNREACHABLE] {
        assert!(
            !diagnostics.iter().any(|d| d.code == code),
            "healthy plan should not emit {code}: {diagnostics:#?}"
        );
    }
}

#[test]
fn doctor_includes_validate_findings_unchanged() {
    // A plan with both an unknown depends-on (validate-only) and a
    // failed predecessor (doctor-only). Doctor must surface BOTH
    // diagnostics, with validate's code unchanged.
    let plan = plan_with(vec![
        change("a", Status::Failed),
        change_with_deps("b", Status::Pending, &["a", "ghost"]),
    ]);
    let diagnostics = doctor(&plan, None, None, None);
    assert!(
        diagnostics.iter().any(|d| d.code == "unknown-depends-on"),
        "validate's `unknown-depends-on` must pass through doctor unchanged: {diagnostics:#?}"
    );
    assert!(
        diagnostics.iter().any(|d| d.code == UNREACHABLE),
        "doctor must add the unreachable diagnostic: {diagnostics:#?}"
    );
}

#[test]
fn diagnostic_serialises_kebab_case() {
    let diag = Diagnostic {
        severity: Severity::Warning,
        code: ORPHAN_SOURCE.to_string(),
        message: "test".into(),
        entry: None,
        data: Some(DiagnosticPayload::OrphanSource {
            key: "monolith".into(),
        }),
    };
    let v = serde_json::to_value(&diag).expect("serialise");
    assert_eq!(v["severity"], "warning");
    assert_eq!(v["code"], ORPHAN_SOURCE);
    assert_eq!(v["data"]["kind"], "orphan-source");
    assert_eq!(v["data"]["key"], "monolith");
}
