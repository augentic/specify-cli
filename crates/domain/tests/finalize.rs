//! Integration tests for `specify_domain::change::finalize`.
//!
//! Substitutes the real `gh` / `git` shell-outs with a `MockCmd`
//! [`CmdRunner`](specify_domain::cmd::CmdRunner) so the run loop's
//! classification, refusal, and atomic archive behaviours can be
//! exercised end-to-end without a forge.

mod common;

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use specify_domain::change::finalize::{
    Inputs, Landing, Outcome, ProjectResult, Refusal, classify_pr, combine, is_terminal,
    outstanding, run, summarise,
};
use specify_domain::change::{Entry, Plan, Status};
use specify_domain::registry::forge::{PrState, PrView};
use specify_domain::registry::{Registry, RegistryProject};
use tempfile::TempDir;

use crate::common::{MockCmd, RecordedCall, fail_stderr, ok_stdout};

// ---- pure helpers -----------------------------------------------------

#[test]
fn terminal_states_accept_done_failed_skipped() {
    assert!(is_terminal(Status::Done));
    assert!(is_terminal(Status::Failed));
    assert!(is_terminal(Status::Skipped));
}

#[test]
fn terminal_states_reject_pending_in_progress_blocked() {
    assert!(!is_terminal(Status::Pending));
    assert!(!is_terminal(Status::InProgress));
    assert!(!is_terminal(Status::Blocked));
}

#[test]
fn classify_pr_no_pr_is_no_branch() {
    assert_eq!(classify_pr(None, "specify/foo"), Landing::NoBranch);
}

#[test]
fn classify_pr_branch_mismatch() {
    let pr = pr_view("feature/x", PrState::Open, false);
    assert_eq!(classify_pr(Some(&pr), "specify/foo"), Landing::BranchPatternMismatch,);
}

#[test]
fn classify_pr_merged_short_circuits() {
    let pr = pr_view("specify/foo", PrState::Merged, true);
    assert_eq!(classify_pr(Some(&pr), "specify/foo"), Landing::Merged);
}

#[test]
fn classify_pr_closed_without_merge() {
    let pr = pr_view("specify/foo", PrState::Closed, false);
    assert_eq!(classify_pr(Some(&pr), "specify/foo"), Landing::Closed);
}

#[test]
fn classify_pr_open_is_unmerged() {
    let pr = pr_view("specify/foo", PrState::Open, false);
    assert_eq!(classify_pr(Some(&pr), "specify/foo"), Landing::Unmerged);
}

#[test]
fn combine_dirty_overrides_passing() {
    assert_eq!(combine(Landing::Merged, true), Landing::Dirty,);
    assert_eq!(combine(Landing::NoBranch, true), Landing::Dirty,);
}

#[test]
fn combine_failed_takes_precedence_over_dirty() {
    assert_eq!(combine(Landing::Failed, true), Landing::Failed,);
}

#[test]
fn combine_clean_passes_through() {
    assert_eq!(combine(Landing::Merged, false), Landing::Merged,);
    assert_eq!(combine(Landing::Unmerged, false), Landing::Unmerged,);
}

#[test]
fn outstanding_lists_in_plan_order() {
    let plan = Plan {
        name: "demo".to_string(),
        sources: BTreeMap::new(),
        entries: vec![
            entry("a", Status::Done),
            entry("b", Status::Pending),
            entry("c", Status::InProgress),
            entry("d", Status::Done),
            entry("e", Status::Blocked),
        ],
    };
    assert_eq!(outstanding(&plan), vec!["b", "c", "e"]);
}

#[test]
fn outstanding_empty_when_all_terminal() {
    let plan = Plan {
        name: "demo".to_string(),
        sources: BTreeMap::new(),
        entries: vec![
            entry("a", Status::Done),
            entry("b", Status::Failed),
            entry("c", Status::Skipped),
        ],
    };
    assert!(outstanding(&plan).is_empty());
}

fn entry(name: &str, status: Status) -> Entry {
    Entry {
        name: name.to_string(),
        project: None,
        capability: Some("omnia@v1".to_string()),
        status,
        depends_on: Vec::new(),
        sources: Vec::new(),
        context: Vec::new(),
        description: None,
        status_reason: None,
    }
}

fn pr_view(branch: &str, state: PrState, merged: bool) -> PrView {
    PrView {
        state,
        merged,
        head_ref_name: branch.to_string(),
        number: 42,
        url: format!("https://github.com/org/repo/pull/{}", 42),
    }
}

// ---- finalize-flavoured MockCmd builder -----------------------------
//
// Per-branch responses for `gh pr list/view` plus per-path dirty flags
// for `git status --porcelain`. Defaults (no `with_view`/`with_dirty`)
// are "no PR" / "clean".

#[derive(Default)]
struct FinalizeMock {
    view: HashMap<String, Result<Option<PrView>, String>>,
    dirty: HashMap<PathBuf, bool>,
}

impl FinalizeMock {
    fn new() -> Self {
        Self::default()
    }

    fn with_view(mut self, branch: &str, view: Result<Option<PrView>, String>) -> Self {
        self.view.insert(branch.to_string(), view);
        self
    }

    fn with_dirty(mut self, path: PathBuf, dirty: bool) -> Self {
        self.dirty.insert(path, dirty);
        self
    }

    fn into_runner(self) -> MockCmd {
        let view = self.view;
        let dirty = self.dirty;
        // Map PR number → owned PrView so `gh pr view <number>` can look
        // up the canned payload.
        let mut by_number: HashMap<u64, PrView> = HashMap::new();
        for v in view.values() {
            if let Ok(Some(pr)) = v {
                by_number.insert(pr.number, pr.clone());
            }
        }
        MockCmd::new(move |call: &RecordedCall| match call.program.as_str() {
            "gh" => dispatch_gh(call, &view, &by_number),
            "git" => dispatch_git(call, &dirty),
            other => panic!("unexpected runner invocation: program={other}, args={:?}", call.args),
        })
    }
}

fn dispatch_gh(
    call: &RecordedCall, view: &HashMap<String, Result<Option<PrView>, String>>,
    by_number: &HashMap<u64, PrView>,
) -> std::io::Result<std::process::Output> {
    match call.args.as_slice() {
        [first, second, third, branch, ..]
            if first == "pr" && second == "list" && third == "--head" =>
        {
            match view.get(branch).cloned().unwrap_or(Ok(None)) {
                Ok(None) => ok_stdout("[]\n"),
                Ok(Some(pr)) => ok_stdout(&format!("[{{\"number\":{}}}]\n", pr.number)),
                Err(msg) => fail_stderr(&msg),
            }
        }
        [first, second, number_str, ..] if first == "pr" && second == "view" => {
            let number: u64 = number_str.parse().expect("pr view <n>");
            let pr = by_number.get(&number).expect("canned pr view");
            let json = serde_json::to_string(pr).expect("serialize PrView");
            ok_stdout(&format!("{json}\n"))
        }
        other => panic!("unexpected gh invocation: {other:?}"),
    }
}

fn dispatch_git(
    call: &RecordedCall, dirty: &HashMap<PathBuf, bool>,
) -> std::io::Result<std::process::Output> {
    // `is_dirty` builds `git -C <path> status --porcelain`.
    match call.args.as_slice() {
        [dash_c, path, status, porcelain]
            if dash_c == "-C" && status == "status" && porcelain == "--porcelain" =>
        {
            let is_dirty = dirty.get(Path::new(path)).copied().unwrap_or(false);
            if is_dirty { ok_stdout(" M file.txt\n") } else { ok_stdout("") }
        }
        other => panic!("unexpected git invocation: {other:?}"),
    }
}

fn registry_with(names: &[&str]) -> Registry {
    Registry {
        version: 1,
        projects: names
            .iter()
            .map(|n| RegistryProject {
                name: (*n).to_string(),
                url: format!("git@github.com:org/{n}.git"),
                capability: "omnia@v1".to_string(),
                description: Some(format!("{n} service")),
                contracts: None,
            })
            .collect(),
    }
}

fn plan_named(name: &str) -> Plan {
    Plan {
        name: name.to_string(),
        sources: BTreeMap::new(),
        entries: Vec::new(),
    }
}

fn plan_with_entries(name: &str, entries: Vec<Entry>) -> Plan {
    Plan {
        name: name.to_string(),
        sources: BTreeMap::new(),
        entries,
    }
}

// ---- guard: non-terminal entries -------------------------------------

#[test]
fn refuses_when_plan_has_outstanding() {
    let tmp = TempDir::new().expect("tempdir");
    let plan =
        plan_with_entries("foo", vec![entry("a", Status::Done), entry("b", Status::Pending)]);
    let registry = registry_with(&["alpha"]);
    let runner = FinalizeMock::new().into_runner();
    let inputs = Inputs {
        project_dir: tmp.path(),
        plan: &plan,
        registry: &registry,
        clean: false,
        dry_run: false,
        now: chrono::Utc::now(),
    };
    let err = run(inputs, &runner).expect_err("non-terminal must refuse");
    assert!(matches!(err, Refusal::NonTerminalEntries(ref names) if names == &["b"]));
    // Probe must not have been called — guard runs before any IO.
    assert!(runner.calls.borrow().is_empty(), "no probes on non-terminal refusal");
}

// ---- guard: per-project PR states ------------------------------------

#[test]
fn finalizes_with_no_clones_and_no_registry_passes() {
    // Edge case: plan has no entries (vacuously terminal) and the
    // registry has no projects. The archive path is still
    // exercised — finalize must succeed.
    let tmp = TempDir::new().expect("tempdir");
    seed_specify_dir(tmp.path());
    let plan_path = tmp.path().join("plan.yaml");
    fs::write(&plan_path, "name: foo\nslices: []\n").expect("seed plan");

    let plan = plan_named("foo");
    let registry = Registry {
        version: 1,
        projects: vec![],
    };
    let runner = FinalizeMock::new().into_runner();
    let inputs = Inputs {
        project_dir: tmp.path(),
        plan: &plan,
        registry: &registry,
        clean: false,
        dry_run: false,
        now: chrono::Utc::now(),
    };
    let outcome = run(inputs, &runner).expect("ok");
    assert!(outcome.finalized);
    assert!(outcome.projects.is_empty());
    assert!(outcome.archived.is_some(), "archive must have run");
    assert!(!plan_path.exists(), "plan.yaml must have moved into archive");
}

#[test]
fn refuses_when_one_project_pr_is_unmerged() {
    let tmp = TempDir::new().expect("tempdir");
    seed_specify_dir(tmp.path());
    let plan_path = tmp.path().join("plan.yaml");
    fs::write(&plan_path, "name: foo\nslices: []\n").expect("seed plan");

    let plan = plan_named("foo");
    let registry = registry_with(&["alpha"]);
    let runner = FinalizeMock::new()
        .with_view(
            "specify/foo",
            Ok(Some(PrView {
                state: PrState::Open,
                merged: false,
                head_ref_name: "specify/foo".to_string(),
                number: 7,
                url: "https://github.com/org/alpha/pull/7".to_string(),
            })),
        )
        .into_runner();
    let inputs = Inputs {
        project_dir: tmp.path(),
        plan: &plan,
        registry: &registry,
        clean: false,
        dry_run: false,
        now: chrono::Utc::now(),
    };
    let outcome = run(inputs, &runner).expect("ok");
    assert!(!outcome.finalized);
    assert_eq!(outcome.projects[0].status, Landing::Unmerged);
    assert_eq!(outcome.projects[0].pr_number, Some(7));
    assert!(
        outcome.projects[0]
            .detail
            .as_deref()
            .is_some_and(|d| d.contains("operator-merge") && d.contains("gh pr merge")),
        "unmerged diagnostic must tell the operator how to land the PR, got: {:?}",
        outcome.projects[0].detail,
    );
    assert!(
        !outcome.projects[0].detail.as_deref().unwrap_or("").contains("workspace merge"),
        "finalize must not point operators at workspace merge automation",
    );
    assert!(
        outcome.message.as_deref().is_some_and(|m| m.contains("operator-merged")),
        "JSON/text summary message must mention operator-merged PRs, got: {:?}",
        outcome.message,
    );
    let json = serde_json::to_value(&outcome).expect("serialize outcome");
    assert!(
        json["message"].as_str().is_some_and(|m| m.contains("operator-merged")),
        "JSON outcome must carry operator-merge guidance, got: {json}",
    );
    assert!(outcome.archived.is_none(), "archive must not run when project refuses");
    // Atomicity: plan.yaml must still exist on refusal.
    assert!(plan_path.exists(), "plan.yaml must remain on disk when finalize refuses");
}

#[test]
fn passes_when_pr_is_merged() {
    let tmp = TempDir::new().expect("tempdir");
    seed_specify_dir(tmp.path());
    fs::write(tmp.path().join("plan.yaml"), "name: foo\nslices: []\n").unwrap();

    let plan = plan_named("foo");
    let registry = registry_with(&["alpha"]);
    let runner = FinalizeMock::new()
        .with_view(
            "specify/foo",
            Ok(Some(PrView {
                state: PrState::Merged,
                merged: true,
                head_ref_name: "specify/foo".to_string(),
                number: 42,
                url: "u".to_string(),
            })),
        )
        .into_runner();
    let inputs = Inputs {
        project_dir: tmp.path(),
        plan: &plan,
        registry: &registry,
        clean: false,
        dry_run: false,
        now: chrono::Utc::now(),
    };
    let outcome = run(inputs, &runner).expect("ok");
    assert!(outcome.finalized);
    assert_eq!(outcome.projects[0].status, Landing::Merged);
    assert_eq!(outcome.summary.merged, 1);
}

#[test]
fn passes_when_no_branch_for_project() {
    let tmp = TempDir::new().expect("tempdir");
    seed_specify_dir(tmp.path());
    fs::write(tmp.path().join("plan.yaml"), "name: foo\nslices: []\n").unwrap();

    let plan = plan_named("foo");
    let registry = registry_with(&["alpha"]);
    // No `with_view` — defaults to Ok(None) i.e. no PR.
    let runner = FinalizeMock::new().into_runner();
    let inputs = Inputs {
        project_dir: tmp.path(),
        plan: &plan,
        registry: &registry,
        clean: false,
        dry_run: false,
        now: chrono::Utc::now(),
    };
    let outcome = run(inputs, &runner).expect("ok");
    assert!(outcome.finalized);
    assert_eq!(outcome.projects[0].status, Landing::NoBranch);
    assert_eq!(outcome.summary.no_branch, 1);
}

#[test]
fn refuses_on_branch_pattern_mismatch() {
    let tmp = TempDir::new().expect("tempdir");
    seed_specify_dir(tmp.path());

    let plan = plan_named("foo");
    let registry = registry_with(&["alpha"]);
    let runner = FinalizeMock::new()
        .with_view(
            "specify/foo",
            Ok(Some(PrView {
                state: PrState::Open,
                merged: false,
                head_ref_name: "feature/foo".to_string(),
                number: 1,
                url: "u".to_string(),
            })),
        )
        .into_runner();
    let inputs = Inputs {
        project_dir: tmp.path(),
        plan: &plan,
        registry: &registry,
        clean: false,
        dry_run: false,
        now: chrono::Utc::now(),
    };
    let outcome = run(inputs, &runner).expect("ok");
    assert!(!outcome.finalized);
    assert_eq!(outcome.projects[0].status, Landing::BranchPatternMismatch);
    // Diagnostic must surface the literal expected branch.
    assert!(
        outcome.projects[0].detail.as_deref().is_some_and(|d| d.contains("specify/foo")),
        "branch-pattern-mismatch detail must include the expected branch, got: {:?}",
        outcome.projects[0].detail,
    );
    assert!(
        outcome.message.as_deref().is_some_and(|m| m.contains("wrong head branch")),
        "summary message must include branch mismatch guidance, got: {:?}",
        outcome.message,
    );
}

#[test]
fn refuses_on_gh_shell_error() {
    let tmp = TempDir::new().expect("tempdir");
    seed_specify_dir(tmp.path());

    let plan = plan_named("foo");
    let registry = registry_with(&["alpha"]);
    let runner = FinalizeMock::new()
        .with_view("specify/foo", Err("simulated gh failure".to_string()))
        .into_runner();
    let inputs = Inputs {
        project_dir: tmp.path(),
        plan: &plan,
        registry: &registry,
        clean: false,
        dry_run: false,
        now: chrono::Utc::now(),
    };
    let outcome = run(inputs, &runner).expect("ok");
    assert!(!outcome.finalized);
    assert_eq!(outcome.projects[0].status, Landing::Failed);
    assert!(outcome.projects[0].detail.as_deref().is_some_and(|d| d.contains("simulated")));
}

// ---- guard: dirty workspace ------------------------------------------

#[test]
fn refuses_dirty_workspace_without_clean() {
    let tmp = TempDir::new().expect("tempdir");
    seed_specify_dir(tmp.path());
    let workspace_base = tmp.path().join(".specify/workspace");
    let alpha_path = workspace_base.join("alpha");
    fs::create_dir_all(&alpha_path).expect("mkdir alpha");

    let plan = plan_named("foo");
    let registry = registry_with(&["alpha"]);
    let runner = FinalizeMock::new()
        .with_view(
            "specify/foo",
            Ok(Some(PrView {
                state: PrState::Merged,
                merged: true,
                head_ref_name: "specify/foo".to_string(),
                number: 42,
                url: "u".to_string(),
            })),
        )
        .with_dirty(alpha_path, true)
        .into_runner();
    let inputs = Inputs {
        project_dir: tmp.path(),
        plan: &plan,
        registry: &registry,
        clean: false,
        dry_run: false,
        now: chrono::Utc::now(),
    };
    let outcome = run(inputs, &runner).expect("ok");
    assert!(!outcome.finalized);
    assert_eq!(outcome.projects[0].status, Landing::Dirty);
    assert_eq!(outcome.projects[0].dirty, Some(true));
    assert!(
        outcome.projects[0].detail.as_deref().is_some_and(|d| d.contains("uncommitted")),
        "dirty diagnostic must mention uncommitted work, got: {:?}",
        outcome.projects[0].detail,
    );
    // Without --clean, the diagnostic should NOT mention --clean would drop work.
    assert!(
        !outcome.projects[0].detail.as_deref().unwrap_or("").contains("--clean"),
        "without --clean, diagnostic should not mention the --clean drop warning",
    );
}

#[test]
fn refuses_dirty_workspace_with_clean() {
    let tmp = TempDir::new().expect("tempdir");
    seed_specify_dir(tmp.path());
    let plan_path = tmp.path().join("plan.yaml");
    fs::write(&plan_path, "name: foo\nslices: []\n").expect("seed plan");
    let workspace_base = tmp.path().join(".specify/workspace");
    let alpha_path = workspace_base.join("alpha");
    let beta_path = workspace_base.join("beta");
    fs::create_dir_all(&alpha_path).expect("mkdir alpha");
    fs::create_dir_all(&beta_path).expect("mkdir beta");

    let plan = plan_named("foo");
    let registry = registry_with(&["alpha", "beta"]);
    let runner = FinalizeMock::new()
        .with_view(
            "specify/foo",
            Ok(Some(PrView {
                state: PrState::Merged,
                merged: true,
                head_ref_name: "specify/foo".to_string(),
                number: 42,
                url: "u".to_string(),
            })),
        )
        .with_dirty(alpha_path.clone(), true)
        .with_dirty(beta_path.clone(), false)
        .into_runner();
    let inputs = Inputs {
        project_dir: tmp.path(),
        plan: &plan,
        registry: &registry,
        clean: true,
        dry_run: false,
        now: chrono::Utc::now(),
    };
    let outcome = run(inputs, &runner).expect("ok");
    assert!(!outcome.finalized);
    assert_eq!(outcome.projects[0].status, Landing::Dirty);
    assert_eq!(
        outcome.projects[1].status,
        Landing::Merged,
        "clean projects may still classify as merged, but any dirty clone blocks the whole run",
    );
    // With --clean, the diagnostic MUST mention that --clean would drop changes.
    assert!(
        outcome.projects[0].detail.as_deref().is_some_and(|d| d.contains("--clean")),
        "with --clean, diagnostic must warn about dropping changes, got: {:?}",
        outcome.projects[0].detail,
    );
    assert!(outcome.cleaned.is_empty(), "refused --clean must report no cleaned clones");
    assert!(plan_path.exists(), "refused --clean must not archive the plan");
    // Workspace clones must still exist — any dirty clone refuses before cleaning any clone.
    assert!(alpha_path.exists(), "refused --clean must leave clones alone");
    assert!(beta_path.exists(), "refused --clean must leave clean clones alone too");
}

// ---- dry-run --------------------------------------------------------

#[test]
fn dry_run_does_not_archive_or_clean() {
    let tmp = TempDir::new().expect("tempdir");
    seed_specify_dir(tmp.path());
    let plan_path = tmp.path().join("plan.yaml");
    fs::write(&plan_path, "name: foo\nslices: []\n").expect("seed plan");
    let workspace_base = tmp.path().join(".specify/workspace");
    let alpha_path = workspace_base.join("alpha");
    fs::create_dir_all(&alpha_path).expect("mkdir alpha");

    let plan = plan_named("foo");
    let registry = registry_with(&["alpha"]);
    let runner = FinalizeMock::new()
        .with_view(
            "specify/foo",
            Ok(Some(PrView {
                state: PrState::Merged,
                merged: true,
                head_ref_name: "specify/foo".to_string(),
                number: 7,
                url: "u".to_string(),
            })),
        )
        .into_runner();
    let inputs = Inputs {
        project_dir: tmp.path(),
        plan: &plan,
        registry: &registry,
        clean: true,
        dry_run: true,
        now: chrono::Utc::now(),
    };
    let outcome = run(inputs, &runner).expect("ok");
    assert!(outcome.finalized, "dry-run with all-passing must report finalized=true");
    assert_eq!(outcome.dry_run, Some(true));
    assert!(outcome.archived.is_none(), "dry-run must not archive");
    assert!(outcome.cleaned.is_empty(), "dry-run must not clean");
    // On-disk state must be unchanged.
    assert!(plan_path.exists(), "dry-run must leave plan.yaml on disk");
    assert!(alpha_path.exists(), "dry-run must leave workspace clones");
}

#[test]
fn dry_run_with_unmerged_pr_reports_not_finalized() {
    let tmp = TempDir::new().expect("tempdir");
    seed_specify_dir(tmp.path());

    let plan = plan_named("foo");
    let registry = registry_with(&["alpha"]);
    let runner = FinalizeMock::new()
        .with_view(
            "specify/foo",
            Ok(Some(PrView {
                state: PrState::Open,
                merged: false,
                head_ref_name: "specify/foo".to_string(),
                number: 7,
                url: "u".to_string(),
            })),
        )
        .into_runner();
    let inputs = Inputs {
        project_dir: tmp.path(),
        plan: &plan,
        registry: &registry,
        clean: false,
        dry_run: true,
        now: chrono::Utc::now(),
    };
    let outcome = run(inputs, &runner).expect("ok");
    assert!(!outcome.finalized);
    assert_eq!(outcome.projects[0].status, Landing::Unmerged);
    assert_eq!(outcome.dry_run, Some(true));
}

// ---- --clean ---------------------------------------------------------

#[test]
fn clean_removes_clones_after_archive() {
    let tmp = TempDir::new().expect("tempdir");
    seed_specify_dir(tmp.path());
    let plan_path = tmp.path().join("plan.yaml");
    fs::write(&plan_path, "name: foo\nslices: []\n").expect("seed plan");
    let workspace_base = tmp.path().join(".specify/workspace");
    let alpha_path = workspace_base.join("alpha");
    fs::create_dir_all(&alpha_path).expect("mkdir alpha");
    // Drop a file inside so remove_dir_all has something to clear.
    fs::write(alpha_path.join("README.md"), "stub\n").expect("seed file");

    let plan = plan_named("foo");
    let registry = registry_with(&["alpha"]);
    let runner = FinalizeMock::new()
        .with_view(
            "specify/foo",
            Ok(Some(PrView {
                state: PrState::Merged,
                merged: true,
                head_ref_name: "specify/foo".to_string(),
                number: 7,
                url: "u".to_string(),
            })),
        )
        .into_runner();
    let inputs = Inputs {
        project_dir: tmp.path(),
        plan: &plan,
        registry: &registry,
        clean: true,
        dry_run: false,
        now: chrono::Utc::now(),
    };
    let outcome = run(inputs, &runner).expect("ok");
    assert!(outcome.finalized);
    assert_eq!(outcome.cleaned, vec!["alpha"], "alpha must be cleaned");
    assert!(!alpha_path.exists(), "workspace clone must be gone");
    assert!(!plan_path.exists(), "plan.yaml must be archived");
}

#[test]
fn clean_waits_until_archive_succeeds() {
    let tmp = TempDir::new().expect("tempdir");
    seed_specify_dir(tmp.path());
    let plan_path = tmp.path().join("plan.yaml");
    fs::write(&plan_path, "name: foo\nslices: []\n").expect("seed plan");
    let archive_root = tmp.path().join(".specify/archive/plans");
    fs::create_dir_all(&archive_root).expect("mkdir archive");
    fs::write(
        archive_root.join(format!("foo-{}.yaml", today_yyyymmdd())),
        "pre-existing archive\n",
    )
    .expect("seed archive collision");
    let workspace_base = tmp.path().join(".specify/workspace");
    let alpha_path = workspace_base.join("alpha");
    fs::create_dir_all(&alpha_path).expect("mkdir alpha");
    fs::write(alpha_path.join("README.md"), "stub\n").expect("seed clone file");

    let plan = plan_named("foo");
    let registry = registry_with(&["alpha"]);
    let runner = FinalizeMock::new()
        .with_view(
            "specify/foo",
            Ok(Some(PrView {
                state: PrState::Merged,
                merged: true,
                head_ref_name: "specify/foo".to_string(),
                number: 7,
                url: "u".to_string(),
            })),
        )
        .into_runner();
    let inputs = Inputs {
        project_dir: tmp.path(),
        plan: &plan,
        registry: &registry,
        clean: true,
        dry_run: false,
        now: chrono::Utc::now(),
    };
    let outcome = run(inputs, &runner).expect("ok");
    assert!(!outcome.finalized, "archive collision must refuse finalize");
    assert!(outcome.cleaned.is_empty(), "failed archive must not clean clones");
    assert!(alpha_path.exists(), "clone must remain when archive fails");
    assert!(plan_path.exists(), "plan.yaml must remain when archive fails");
    assert!(
        outcome.message.as_deref().is_some_and(|m| m.contains("archive failed")),
        "archive failure should produce a summary message, got: {:?}",
        outcome.message,
    );
    assert!(
        outcome.projects[0].detail.as_deref().is_some_and(|d| d.contains("plan archive failed")),
        "archive failure detail should be attached to the first project row, got: {:?}",
        outcome.projects[0].detail,
    );
}

#[test]
fn clean_skips_symlink_projects() {
    let tmp = TempDir::new().expect("tempdir");
    seed_specify_dir(tmp.path());
    fs::write(tmp.path().join("plan.yaml"), "name: foo\nslices: []\n").unwrap();

    let plan = plan_named("foo");
    // url: "." → symlink-mode; clean must not delete the project_dir.
    let registry = Registry {
        version: 1,
        projects: vec![RegistryProject {
            name: "alpha".to_string(),
            url: ".".to_string(),
            capability: "omnia@v1".to_string(),
            description: Some("alpha service".to_string()),
            contracts: None,
        }],
    };
    let runner = FinalizeMock::new().into_runner();
    let inputs = Inputs {
        project_dir: tmp.path(),
        plan: &plan,
        registry: &registry,
        clean: true,
        dry_run: false,
        now: chrono::Utc::now(),
    };
    let outcome = run(inputs, &runner).expect("ok");
    assert!(outcome.finalized);
    assert!(outcome.cleaned.is_empty(), "symlink projects must not be cleaned");
}

// ---- idempotency -----------------------------------------------------

/// Operator runs finalize once with one PR open, gets refused.
/// Operator merges the PR by hand. Operator runs finalize again —
/// archive completes. The fixture verifies the second-run path.
#[test]
fn idempotent_after_manual_merge() {
    let tmp = TempDir::new().expect("tempdir");
    seed_specify_dir(tmp.path());
    let plan_path = tmp.path().join("plan.yaml");
    fs::write(&plan_path, "name: foo\nslices: []\n").expect("seed plan");

    let plan = plan_named("foo");
    let registry = registry_with(&["alpha"]);

    // First run: PR open, finalize refuses.
    let runner1 = FinalizeMock::new()
        .with_view(
            "specify/foo",
            Ok(Some(PrView {
                state: PrState::Open,
                merged: false,
                head_ref_name: "specify/foo".to_string(),
                number: 7,
                url: "u".to_string(),
            })),
        )
        .into_runner();
    let outcome1 = run(
        Inputs {
            project_dir: tmp.path(),
            plan: &plan,
            registry: &registry,
            clean: false,
            dry_run: false,
            now: chrono::Utc::now(),
        },
        &runner1,
    )
    .expect("ok");
    assert!(!outcome1.finalized, "first run must refuse on unmerged PR");
    assert!(outcome1.archived.is_none());
    assert!(plan_path.exists(), "plan.yaml must still be present after refusal");

    // Operator merges the PR manually. Re-run finalize against a
    // probe that now reports MERGED — archive must land.
    let runner2 = FinalizeMock::new()
        .with_view(
            "specify/foo",
            Ok(Some(PrView {
                state: PrState::Merged,
                merged: true,
                head_ref_name: "specify/foo".to_string(),
                number: 7,
                url: "u".to_string(),
            })),
        )
        .into_runner();
    let outcome2 = run(
        Inputs {
            project_dir: tmp.path(),
            plan: &plan,
            registry: &registry,
            clean: false,
            dry_run: false,
            now: chrono::Utc::now(),
        },
        &runner2,
    )
    .expect("ok");
    assert!(outcome2.finalized, "second run after manual merge must finalize");
    assert!(outcome2.archived.is_some());
    assert!(!plan_path.exists(), "plan.yaml must be archived");
}

// ---- summary --------------------------------------------------------

#[test]
fn summary_counts_per_status() {
    let results = vec![
        ProjectResult {
            name: "a".into(),
            status: Landing::Merged,
            pr_number: None,
            url: None,
            head_ref_name: None,
            dirty: None,
            detail: None,
        },
        ProjectResult {
            name: "b".into(),
            status: Landing::NoBranch,
            pr_number: None,
            url: None,
            head_ref_name: None,
            dirty: None,
            detail: None,
        },
        ProjectResult {
            name: "c".into(),
            status: Landing::Unmerged,
            pr_number: None,
            url: None,
            head_ref_name: None,
            dirty: None,
            detail: None,
        },
        ProjectResult {
            name: "d".into(),
            status: Landing::Dirty,
            pr_number: None,
            url: None,
            head_ref_name: None,
            dirty: Some(true),
            detail: None,
        },
    ];
    let s = summarise(&results);
    assert_eq!(s.merged, 1);
    assert_eq!(s.no_branch, 1);
    assert_eq!(s.unmerged, 1);
    assert_eq!(s.dirty, 1);
}

// silence unused-import warnings for Outcome — referenced in doctest-only
// examples below.
#[allow(dead_code)]
const fn _outcome_type_alias() -> Option<Outcome> {
    None
}

// ---- helpers --------------------------------------------------------

/// Seed `<tmp>/.specify/` so `Plan::archive` and friends have a
/// real on-disk parent to operate on.
fn seed_specify_dir(project_dir: &Path) {
    fs::create_dir_all(project_dir.join(".specify")).expect("mkdir .specify");
}

fn today_yyyymmdd() -> String {
    chrono::Utc::now().format("%Y%m%d").to_string()
}
