//! Integration tests for `specify_registry::workspace` and
//! `specify_registry::merge`.
//!
//! Pins the public surface lifted from the binary by RFC-13 chunk 2.2:
//! `extract_github_slug`, `sync_registry_workspace` (registry-absent
//! short-circuit, `.gitignore` upkeep), `workspace_status` (returns
//! `None` when no registry), `matches_specify_branch_pattern`, and
//! `pr_branch_matches`. Per-classifier coverage continues to live in
//! the in-module `#[cfg(test)]` blocks; this file exercises the
//! integration boundary an external consumer (the binary, plus
//! anyone replacing it via `--lib`) would touch.

use std::fs;
use std::path::Path;

use specify_registry::merge::{matches_specify_branch_pattern, pr_branch_matches};
use specify_registry::workspace::{
    SlotKind, SlotStatus, extract_github_slug, sync_registry_workspace, workspace_status,
};
use tempfile::TempDir;

// ---------- extract_github_slug --------------------------------------

#[test]
fn extract_github_slug_handles_each_supported_form() {
    assert_eq!(
        extract_github_slug("git@github.com:org/mobile.git"),
        Some("org/mobile".to_string())
    );
    assert_eq!(
        extract_github_slug("git@github.com:org/mobile"),
        Some("org/mobile".to_string())
    );
    assert_eq!(
        extract_github_slug("https://github.com/org/mobile.git"),
        Some("org/mobile".to_string())
    );
    assert_eq!(
        extract_github_slug("https://github.com/org/mobile"),
        Some("org/mobile".to_string())
    );
    assert_eq!(
        extract_github_slug("ssh://git@github.com/org/mobile.git"),
        Some("org/mobile".to_string())
    );
    assert_eq!(extract_github_slug("git@gitlab.com:org/repo.git"), None);
}

// ---------- sync_registry_workspace ----------------------------------

#[test]
fn sync_registry_workspace_no_registry_is_noop() {
    let tmp = TempDir::new().unwrap();
    sync_registry_workspace(tmp.path()).expect("absent registry must not error");
    // No `.gitignore` written when there is nothing to sync — the
    // helper is only invoked once a registry is present.
    assert!(!tmp.path().join(".gitignore").exists());
    assert!(!tmp.path().join(".specify/workspace").exists());
}

#[test]
fn sync_registry_workspace_with_symlink_entry_creates_workspace_and_gitignore() {
    let tmp = TempDir::new().unwrap();
    let project_dir = tmp.path();

    let peer_dir = project_dir.join("peer");
    fs::create_dir_all(&peer_dir).unwrap();

    fs::write(
        project_dir.join("registry.yaml"),
        "\
version: 1
projects:
  - name: peer
    url: ./peer
    schema: omnia@v1
",
    )
    .unwrap();

    sync_registry_workspace(project_dir).expect("sync ok");

    let slot = project_dir.join(".specify/workspace/peer");
    assert!(slot.exists(), "symlink slot must materialise");
    let meta = fs::symlink_metadata(&slot).unwrap();
    assert!(meta.file_type().is_symlink(), "symlink expected, got {meta:?}");
    let target = fs::canonicalize(&slot).unwrap();
    assert_eq!(target, fs::canonicalize(&peer_dir).unwrap());

    let gitignore = fs::read_to_string(project_dir.join(".gitignore")).unwrap();
    assert!(gitignore.lines().any(|l| l.trim() == ".specify/workspace/"));
    assert!(gitignore.lines().any(|l| l.trim() == ".specify/.cache/"));
}

// ---------- workspace_status -----------------------------------------

#[test]
fn workspace_status_returns_none_without_registry() {
    let tmp = TempDir::new().unwrap();
    let result = workspace_status(tmp.path()).expect("ok");
    assert!(result.is_none(), "absent registry must yield None");
}

#[test]
fn workspace_status_reports_missing_for_unrealised_slot() {
    let tmp = TempDir::new().unwrap();
    let project_dir = tmp.path();
    fs::write(
        project_dir.join("registry.yaml"),
        "\
version: 1
projects:
  - name: alpha
    url: git@github.com:org/alpha.git
    schema: omnia@v1
    description: alpha
  - name: beta
    url: git@github.com:org/beta.git
    schema: omnia@v1
    description: beta
",
    )
    .unwrap();

    let slots = workspace_status(project_dir).expect("ok").expect("registry present");
    assert_eq!(slots.len(), 2);
    assert!(
        slots.iter().all(|s| matches!(
            s,
            SlotStatus {
                kind: SlotKind::Missing,
                head_sha: None,
                dirty: None,
                ..
            }
        )),
        "unmaterialised slots must classify as Missing, got: {slots:?}",
    );
    assert_eq!(slots[0].name, "alpha");
    assert_eq!(slots[1].name, "beta");
}

#[test]
fn workspace_status_reports_symlink_kind_after_sync() {
    let tmp = TempDir::new().unwrap();
    let project_dir = tmp.path();
    fs::create_dir_all(project_dir.join("peer")).unwrap();
    fs::write(
        project_dir.join("registry.yaml"),
        "\
version: 1
projects:
  - name: peer
    url: ./peer
    schema: omnia@v1
",
    )
    .unwrap();

    sync_registry_workspace(project_dir).unwrap();
    let slots = workspace_status(project_dir).unwrap().unwrap();
    assert_eq!(slots.len(), 1);
    assert_eq!(slots[0].name, "peer");
    assert_eq!(slots[0].kind, SlotKind::Symlink);
    // No git work tree behind the symlink target — head/dirty stay None.
    assert!(slots[0].head_sha.is_none());
}

// ---------- merge:: branch matchers ----------------------------------

#[test]
fn merge_branch_matchers_round_trip_canonical_inputs() {
    assert!(matches_specify_branch_pattern("specify/foo"));
    assert!(matches_specify_branch_pattern("specify/platform-v2"));
    assert!(!matches_specify_branch_pattern("feature/bar"));
    assert!(!matches_specify_branch_pattern("specify/foo/bar"));

    assert!(pr_branch_matches("specify/foo", "specify/foo"));
    assert!(!pr_branch_matches("specify/foo", "specify/bar"));
    assert!(!pr_branch_matches("feature/foo", "specify/foo"));
}

// Sanity: the workspace base helper is private but the path it
// computes is observable through `workspace_status` (`.specify/workspace/`).
// This test pins the layout so a future refactor to the private helper
// keeps producing the same on-disk path the binary expects.
#[test]
fn sync_lays_down_workspace_under_dot_specify() {
    let tmp = TempDir::new().unwrap();
    let project_dir = tmp.path();
    fs::create_dir_all(project_dir.join("peer")).unwrap();
    fs::write(
        project_dir.join("registry.yaml"),
        "\
version: 1
projects:
  - name: peer
    url: ./peer
    schema: omnia@v1
",
    )
    .unwrap();

    sync_registry_workspace(project_dir).unwrap();
    assert!(Path::new(&project_dir.join(".specify/workspace")).is_dir());
}
