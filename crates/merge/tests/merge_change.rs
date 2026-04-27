//! End-to-end filesystem tests for `merge_change`.
//!
//! Each test builds a throw-away project under `tempfile::TempDir`, seeds a
//! change directory with delta specs at `specs/<name>/spec.md`, and drives
//! `merge_change` through its happy + sad paths. Discovery is
//! convention-based — no schema or `generates` directive is needed.

use std::fs;
use std::path::PathBuf;

use regex::Regex;
use specify_change::{ChangeMetadata, LifecycleStatus, Outcome, Phase, PhaseOutcome};
use specify_error::Error;
use specify_merge::{ContractAction, merge_change, preview_change};
use tempfile::TempDir;

const CHANGE_NAME: &str = "feature-x";

struct Project {
    _tmp: TempDir,
    root: PathBuf,
}

impl Project {
    fn change_dir(&self) -> PathBuf {
        self.root.join(".specify/changes").join(CHANGE_NAME)
    }

    fn specs_dir(&self) -> PathBuf {
        self.root.join(".specify/specs")
    }

    fn archive_dir(&self) -> PathBuf {
        self.root.join(".specify/archive")
    }
}

/// Build a fixture project with delta specs at the conventional path.
/// No schema directory or merge brief is needed — discovery scans
/// `<change_dir>/specs/*/spec.md` directly.
fn build_project() -> Project {
    let tmp = TempDir::new().expect("tempdir");
    let root = tmp.path().to_path_buf();

    for sub in [".specify/changes", ".specify/specs", ".specify/archive"] {
        fs::create_dir_all(root.join(sub)).expect("mkdir");
    }

    let change_dir = root.join(".specify/changes").join(CHANGE_NAME);
    fs::create_dir_all(change_dir.join("specs/login")).expect("mkdir login");
    fs::create_dir_all(change_dir.join("specs/oauth")).expect("mkdir oauth");
    fs::write(change_dir.join("proposal.md"), "# proposal\n").expect("write proposal");
    fs::write(change_dir.join("specs/login/spec.md"), include_str!("data/delta-login.md"))
        .expect("write login delta");
    fs::write(change_dir.join("specs/oauth/spec.md"), include_str!("data/delta-oauth.md"))
        .expect("write oauth delta");

    let metadata = ChangeMetadata {
        schema: "omnia".to_string(),
        status: LifecycleStatus::Complete,
        created_at: Some("2024-08-01T10:00:00Z".to_string()),
        defined_at: Some("2024-08-01T12:00:00Z".to_string()),
        build_started_at: Some("2024-08-02T09:30:00Z".to_string()),
        completed_at: None,
        merged_at: None,
        dropped_at: None,
        drop_reason: None,
        touched_specs: vec![],
        outcome: None,
    };
    metadata.save(&change_dir).expect("save metadata");

    Project { _tmp: tmp, root }
}

// ---------------------------------------------------------------------------
// Happy path
// ---------------------------------------------------------------------------

#[test]
fn happy_path_writes_baselines_flips_status_and_archives() {
    let project = build_project();
    let change_dir = project.change_dir();
    let specs_dir = project.specs_dir();
    let archive_dir = project.archive_dir();

    let merged =
        merge_change(&change_dir, &specs_dir, &archive_dir).expect("merge_change should succeed");

    // Results sorted by spec name.
    let names: Vec<&str> = merged.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, vec!["login", "oauth"]);

    // Baselines now exist.
    let login_baseline = specs_dir.join("login/spec.md");
    let oauth_baseline = specs_dir.join("oauth/spec.md");
    assert!(login_baseline.is_file(), "{} missing", login_baseline.display());
    assert!(oauth_baseline.is_file(), "{} missing", oauth_baseline.display());
    let login_text = fs::read_to_string(&login_baseline).unwrap();
    assert!(login_text.contains("REQ-001"));
    assert!(login_text.contains("### Requirement:"));

    // Change directory has moved under archive/.
    assert!(!change_dir.exists(), "{} should be gone", change_dir.display());
    let archive_re = Regex::new(r"^\d{4}-\d{2}-\d{2}-feature-x$").unwrap();
    let archived: Vec<_> = fs::read_dir(&archive_dir)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .collect();
    assert_eq!(archived.len(), 1);
    let archived_name = archived[0].file_name().to_string_lossy().to_string();
    assert!(
        archive_re.is_match(&archived_name),
        "archive dir `{archived_name}` does not match YYYY-MM-DD-feature-x"
    );

    // .metadata.yaml inside archive should be `Merged` with completed_at set.
    let archived_change_dir = archived[0].path();
    let new_meta = ChangeMetadata::load(&archived_change_dir).expect("load archived metadata");
    assert_eq!(new_meta.status, LifecycleStatus::Merged);
    assert!(new_meta.completed_at.is_some(), "expected completed_at to be set after merge");

    // merge_change stamps the phase outcome before archiving.
    let outcome = new_meta.outcome.expect("expected outcome to be stamped by merge_change");
    assert_eq!(outcome.phase, Phase::Merge);
    assert_eq!(outcome.outcome, Outcome::Success);
    assert!(outcome.summary.contains("2 spec(s)"), "unexpected summary: {}", outcome.summary);
}

// ---------------------------------------------------------------------------
// Wrong precondition
// ---------------------------------------------------------------------------

#[test]
fn wrong_precondition_aborts_cleanly() {
    let project = build_project();
    let change_dir = project.change_dir();

    // Re-save metadata with status = Building.
    let mut meta = ChangeMetadata::load(&change_dir).unwrap();
    meta.status = LifecycleStatus::Building;
    meta.save(&change_dir).unwrap();

    let err = merge_change(&change_dir, &project.specs_dir(), &project.archive_dir())
        .expect_err("should refuse on Building status");
    match err {
        Error::Lifecycle { expected, found } => {
            assert_eq!(expected, "Complete");
            assert!(found.contains("Building"), "unexpected found: {found}");
        }
        other => panic!("expected Lifecycle error, got {other:?}"),
    }

    // Filesystem untouched.
    assert!(change_dir.exists(), "change dir must still exist");
    assert!(!project.specs_dir().join("login/spec.md").exists());
    assert!(!project.specs_dir().join("oauth/spec.md").exists());
    assert!(
        fs::read_dir(project.archive_dir()).unwrap().next().is_none(),
        "archive dir must still be empty"
    );
}

// ---------------------------------------------------------------------------
// Coherence failure rollback
// ---------------------------------------------------------------------------

#[test]
fn coherence_failure_rolls_back_all_writes() {
    let project = build_project();
    let change_dir = project.change_dir();

    // Overwrite the login delta with one that produces a coherence-invalid
    // baseline: an ADDED block missing its `ID:` line.
    fs::write(
        change_dir.join("specs/login/spec.md"),
        "## ADDED Requirements\n\n### Requirement: Missing id\n\n#### Scenario: ok\n\n- ok\n",
    )
    .unwrap();

    let err = merge_change(&change_dir, &project.specs_dir(), &project.archive_dir())
        .expect_err("expected coherence failure");
    match err {
        Error::Merge(msg) => {
            assert!(
                msg.contains("login:") && msg.contains("has no ID: line"),
                "unexpected merge error: {msg}"
            );
        }
        other => panic!("expected Error::Merge, got {other:?}"),
    }

    // Nothing on disk has moved or been created.
    assert!(change_dir.exists(), "change dir must still exist");
    let meta = ChangeMetadata::load(&change_dir).unwrap();
    assert_eq!(meta.status, LifecycleStatus::Complete);
    assert!(!project.specs_dir().join("login/spec.md").exists());
    assert!(!project.specs_dir().join("oauth/spec.md").exists());
    assert!(
        fs::read_dir(project.archive_dir()).unwrap().next().is_none(),
        "archive must remain empty"
    );
}

// ---------------------------------------------------------------------------
// Archive naming (already covered by happy path; add a stand-alone assertion
// so a regex-only break doesn't hide inside the bigger happy-path test).
// ---------------------------------------------------------------------------

#[test]
fn archive_subdirectory_is_date_prefixed() {
    let project = build_project();
    merge_change(&project.change_dir(), &project.specs_dir(), &project.archive_dir())
        .expect("merge ok");

    let re = Regex::new(r"^\d{4}-\d{2}-\d{2}-feature-x$").unwrap();
    let names: Vec<String> = fs::read_dir(project.archive_dir())
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    assert!(
        names.iter().any(|n| re.is_match(n)),
        "archive names {names:?} do not include a YYYY-MM-DD-feature-x entry"
    );
}

// ---------------------------------------------------------------------------
// Contract file copying
// ---------------------------------------------------------------------------

#[test]
fn merge_copies_contract_files_to_baseline() {
    let project = build_project();
    let change_dir = project.change_dir();

    fs::create_dir_all(change_dir.join("contracts/schemas")).expect("mkdir schemas");
    fs::create_dir_all(change_dir.join("contracts/http")).expect("mkdir http");
    fs::write(change_dir.join("contracts/schemas/test.yaml"), "schema: test\n")
        .expect("write schema");
    fs::write(change_dir.join("contracts/http/api.yaml"), "openapi: 3.1\n").expect("write api");

    let merged =
        merge_change(&change_dir, &project.specs_dir(), &project.archive_dir()).expect("merge ok");

    let baseline_contracts = project.root.join(".specify/contracts");
    assert!(
        baseline_contracts.join("schemas/test.yaml").is_file(),
        "schemas/test.yaml missing from baseline contracts"
    );
    assert!(
        baseline_contracts.join("http/api.yaml").is_file(),
        "http/api.yaml missing from baseline contracts"
    );

    let archived = find_archived_metadata(&project);
    assert!(
        archived.summary.contains("2 contract file(s)"),
        "unexpected summary: {}",
        archived.summary
    );
    assert!(
        archived.summary.contains(&format!("{} spec(s)", merged.len())),
        "unexpected summary: {}",
        archived.summary
    );
}

#[test]
fn merge_replaces_existing_baseline_contract_files() {
    let project = build_project();
    let change_dir = project.change_dir();

    let baseline_contracts = project.root.join(".specify/contracts");
    fs::create_dir_all(baseline_contracts.join("schemas")).expect("mkdir baseline schemas");
    fs::write(baseline_contracts.join("schemas/test.yaml"), "old content\n")
        .expect("write old baseline");

    fs::create_dir_all(change_dir.join("contracts/schemas")).expect("mkdir change schemas");
    fs::write(change_dir.join("contracts/schemas/test.yaml"), "new content\n")
        .expect("write new change");

    merge_change(&change_dir, &project.specs_dir(), &project.archive_dir()).expect("merge ok");

    let content = fs::read_to_string(baseline_contracts.join("schemas/test.yaml")).unwrap();
    assert_eq!(content, "new content\n", "contract file should be replaced");
}

#[test]
fn merge_leaves_untouched_baseline_contract_files() {
    let project = build_project();
    let change_dir = project.change_dir();

    let baseline_contracts = project.root.join(".specify/contracts");
    fs::create_dir_all(baseline_contracts.join("schemas")).expect("mkdir baseline schemas");
    fs::write(baseline_contracts.join("schemas/existing.yaml"), "existing content\n")
        .expect("write existing");

    fs::create_dir_all(change_dir.join("contracts/schemas")).expect("mkdir change schemas");
    fs::write(change_dir.join("contracts/schemas/new.yaml"), "new content\n")
        .expect("write new");

    merge_change(&change_dir, &project.specs_dir(), &project.archive_dir()).expect("merge ok");

    assert!(
        baseline_contracts.join("schemas/existing.yaml").is_file(),
        "existing contract should still be present"
    );
    assert!(
        baseline_contracts.join("schemas/new.yaml").is_file(),
        "new contract should be present"
    );
    let existing = fs::read_to_string(baseline_contracts.join("schemas/existing.yaml")).unwrap();
    assert_eq!(existing, "existing content\n", "existing contract should be untouched");
}

#[test]
fn merge_without_contracts_dir_works_as_before() {
    let project = build_project();
    let change_dir = project.change_dir();

    assert!(!change_dir.join("contracts").exists(), "precondition: no contracts dir");

    let merged =
        merge_change(&change_dir, &project.specs_dir(), &project.archive_dir()).expect("merge ok");
    assert!(!merged.is_empty(), "should still merge specs");

    let baseline_contracts = project.root.join(".specify/contracts");
    assert!(
        !baseline_contracts.exists(),
        "no .specify/contracts/ should be created when change has no contracts"
    );

    let archived = find_archived_metadata(&project);
    assert!(
        !archived.summary.contains("contract"),
        "summary should not mention contracts: {}",
        archived.summary
    );
}

/// Helper: find the archived `.metadata.yaml` and return its phase outcome.
fn find_archived_metadata(project: &Project) -> PhaseOutcome {
    let archived: Vec<_> = fs::read_dir(project.archive_dir())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .collect();
    assert_eq!(archived.len(), 1, "expected exactly one archived change");
    let meta = ChangeMetadata::load(&archived[0].path()).expect("load archived metadata");
    meta.outcome.expect("expected outcome to be stamped")
}

// ---------------------------------------------------------------------------
// preview_change — contract entries
// ---------------------------------------------------------------------------

#[test]
fn preview_no_contracts_returns_empty_list() {
    let project = build_project();
    let result = preview_change(&project.change_dir(), &project.specs_dir())
        .expect("preview should succeed");
    assert!(!result.specs.is_empty(), "should have spec entries");
    assert!(result.contracts.is_empty(), "should have no contract entries");
}

#[test]
fn preview_new_contract_files_reported_as_added() {
    let project = build_project();
    let change_dir = project.change_dir();

    fs::create_dir_all(change_dir.join("contracts/schemas")).expect("mkdir");
    fs::create_dir_all(change_dir.join("contracts/http")).expect("mkdir");
    fs::write(change_dir.join("contracts/schemas/user.yaml"), "schema: user\n").expect("write");
    fs::write(change_dir.join("contracts/http/api.yaml"), "openapi: 3.1\n").expect("write");

    let result =
        preview_change(&change_dir, &project.specs_dir()).expect("preview should succeed");

    assert_eq!(result.contracts.len(), 2);
    // Sorted by relative_path.
    assert_eq!(result.contracts[0].relative_path, "http/api.yaml");
    assert_eq!(result.contracts[0].action, ContractAction::Added);
    assert_eq!(result.contracts[1].relative_path, "schemas/user.yaml");
    assert_eq!(result.contracts[1].action, ContractAction::Added);
}

#[test]
fn preview_existing_baseline_contracts_reported_as_replaced() {
    let project = build_project();
    let change_dir = project.change_dir();

    let baseline_contracts = project.root.join(".specify/contracts");
    fs::create_dir_all(baseline_contracts.join("schemas")).expect("mkdir baseline");
    fs::write(baseline_contracts.join("schemas/user.yaml"), "old\n").expect("write baseline");

    fs::create_dir_all(change_dir.join("contracts/schemas")).expect("mkdir change");
    fs::write(change_dir.join("contracts/schemas/user.yaml"), "new\n").expect("write change");
    fs::write(change_dir.join("contracts/schemas/order.yaml"), "new\n").expect("write change");

    let result =
        preview_change(&change_dir, &project.specs_dir()).expect("preview should succeed");

    assert_eq!(result.contracts.len(), 2);
    let order = result.contracts.iter().find(|c| c.relative_path == "schemas/order.yaml").unwrap();
    assert_eq!(order.action, ContractAction::Added);
    let user = result.contracts.iter().find(|c| c.relative_path == "schemas/user.yaml").unwrap();
    assert_eq!(user.action, ContractAction::Replaced);
}
