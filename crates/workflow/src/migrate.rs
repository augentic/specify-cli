//! Migration framework (RFC-30 §D3).
//!
//! Holds the closed [`MigrationKind`] registry, the [`Migrator`] trait,
//! the [`MigrationPlan`] / [`MigrationReport`] DTOs, and the shared
//! staged-write-then-rename apply harness ([`apply_staged`]) every
//! migrator reuses.
//!
//! [`MigrationKind::id`] is the single source of truth for a migrator's
//! stable kebab-case id; concrete [`Migrator`] impls echo it from
//! [`Migrator::id`] and stamp it onto every plan and report they emit.
//! This module owns the framework only — the concrete `V1ToV2`
//! transforms and the `specify migrate` command land in their own
//! changes and do not emit journal events from here (the command layer
//! derives `migration.applied` / `migration.skipped` from the returned
//! [`MigrationReport`]).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use specify_digest::sha256_hex;
use specify_error::Error;

use crate::config::Layout;

mod v1_to_v2;

pub use v1_to_v2::V1ToV2;

/// Closed registry of per-major migration paths.
///
/// Adding a major version requires a new variant *and* a registered
/// [`Migrator`] impl *and* a golden fixture under
/// `tests/migrate/<from>-to-<to>/`.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MigrationKind {
    /// 1.x → 2.0 structural migration.
    V1ToV2,
    // V2ToV3 lands when 3.0 is cut.
}

/// One single-major adjacency in the migration walk.
struct Hop {
    /// Major version the hop migrates from.
    from: u64,
    /// Major version the hop migrates to.
    to: u64,
    /// Migrator covering the hop.
    kind: MigrationKind,
}

/// Ordered registry of the known major-version hops. Adding `V2ToV3`
/// is a one-line append here, paired with its [`MigrationKind`] variant
/// and [`Migrator`] impl.
const HOPS: &[Hop] = &[Hop {
    from: 1,
    to: 2,
    kind: MigrationKind::V1ToV2,
}];

impl MigrationKind {
    /// Stable kebab-case id — the single source of truth.
    ///
    /// A [`Migrator`] echoes it from [`Migrator::id`], and it matches
    /// the `kebab-case` serde wire form of the variant.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::V1ToV2 => "v1-to-v2",
        }
    }

    /// Ordered list of migrations to walk major `from` to major `to`.
    ///
    /// Composes one registered hop per major (e.g. `1 → 3` yields
    /// `[V1ToV2, V2ToV3]` once both hops are registered). Empty when
    /// `to <= from`, or when no contiguous chain of registered
    /// migrators reaches `to` (a gap in the hop registry).
    #[must_use]
    pub fn resolve(from: u64, to: u64) -> Vec<Self> {
        let mut chain = Vec::new();
        let mut cursor = from;
        while cursor < to {
            let Some(hop) = HOPS.iter().find(|hop| hop.from == cursor) else {
                return Vec::new();
            };
            chain.push(hop.kind);
            cursor = hop.to;
        }
        if cursor == to { chain } else { Vec::new() }
    }
}

/// Major version component of `version`, or `None` when unparseable.
///
/// Permissive on bad input, matching the config-load stance. The
/// command layer uses it to turn `--from` / `--to` version strings into
/// the majors [`MigrationKind::resolve`] walks.
#[must_use]
pub fn major(version: &str) -> Option<u64> {
    semver::Version::parse(version).ok().map(|v| v.major)
}

/// A registered migrator covering one [`MigrationKind`] hop.
///
/// Object-safe so the command layer can dispatch over `&dyn Migrator`.
pub trait Migrator {
    /// Stable kebab-case id, e.g. `v1-to-v2`. Concrete impls return
    /// their [`MigrationKind::id`].
    fn id(&self) -> &'static str;

    /// Inspect the project and return the file actions WITHOUT applying
    /// them. Used by `--dry-run` and by `specify init --check-migration`.
    ///
    /// # Errors
    ///
    /// Propagates inspection failures (I/O, malformed source artifacts).
    fn plan(&self, project_dir: &Path) -> Result<MigrationPlan, Error>;

    /// Apply the plan atomically (staged write + rename). Returns a
    /// report with per-file checksums and a top-level status. Concrete
    /// impls delegate to [`apply_staged`].
    ///
    /// # Errors
    ///
    /// Propagates staging / commit failures; a precondition failure
    /// leaves the project untouched.
    fn apply(&self, project_dir: &Path, plan: &MigrationPlan) -> Result<MigrationReport, Error>;
}

/// Resolve a [`MigrationKind`] to its registered [`Migrator`].
///
/// The command layer (`specify migrate`, `specify init
/// --check-migration`) maps each kind [`MigrationKind::resolve`]
/// returns through this function to drive `plan` / `apply`. The match
/// is exhaustive over the closed [`MigrationKind`] enum, so a new hop
/// cannot be registered without also registering its migrator here.
#[must_use]
pub fn migrator_for(kind: MigrationKind) -> &'static dyn Migrator {
    match kind {
        MigrationKind::V1ToV2 => &V1ToV2,
    }
}

/// One `(kind, plan)` pair from the read-only migration [`probe`].
///
/// The command layer maps each entry into the `init
/// --check-migration` JSON envelope; an entry with an empty
/// [`MigrationPlan::actions`] means the hop is registered but the
/// project is already at the target shape (no work to do).
#[derive(Debug, Clone)]
pub struct ProbedMigration {
    /// Registered migrator the probe inspected.
    pub kind: MigrationKind,
    /// Pure (no-write) plan the migrator would apply.
    pub plan: MigrationPlan,
}

/// Read-only probe walking major `from` to major `to` over
/// `project_dir`.
///
/// Resolves the hop chain via [`MigrationKind::resolve`] and runs each
/// migrator's pure [`Migrator::plan`] (no writes), returning one
/// [`ProbedMigration`] per hop. Empty when no contiguous chain of
/// registered migrators reaches `to` (including `to <= from`), which
/// the `specify init --check-migration` command maps to
/// `needs-migration: false`. Used only by that probe; the
/// `specify migrate` command drives `plan` / `apply` directly.
///
/// # Errors
///
/// Propagates any migrator [`Migrator::plan`] inspection failure.
pub fn probe(project_dir: &Path, from: u64, to: u64) -> Result<Vec<ProbedMigration>, Error> {
    MigrationKind::resolve(from, to)
        .into_iter()
        .map(|kind| migrator_for(kind).plan(project_dir).map(|plan| ProbedMigration { kind, plan }))
        .collect()
}

/// One file action in a [`MigrationPlan`].
///
/// `#[non_exhaustive]`: concrete migrators may add variants (e.g. a
/// removal for the monolithic-`adapter.yaml` split). Structured edits
/// are expressed as a [`MigrationAction::Rewrite`] carrying the
/// migrator-computed post-edit contents, so the shared apply harness
/// stays a dumb byte mover.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum MigrationAction {
    /// Relocate a file from one project-relative path to another with
    /// no content change.
    Move {
        /// Project-relative source path.
        from: PathBuf,
        /// Project-relative destination path.
        to: PathBuf,
    },
    /// Replace a file's full contents, creating it when absent.
    Rewrite {
        /// Project-relative path to write.
        path: PathBuf,
        /// Resulting file contents.
        contents: String,
    },
    /// Delete a file from the live tree. Used by the `V1ToV2`
    /// adapter-split transform to drop the monolithic `adapter.yaml`
    /// after its axis-split replacement has been written — a
    /// [`MigrationAction::Move`] cannot delete, and a
    /// [`MigrationAction::Rewrite`] cannot either, so removal is its
    /// own action. The source path must exist; a missing target aborts
    /// the apply before any mutation, like a missing move source.
    Remove {
        /// Project-relative path to delete.
        path: PathBuf,
    },
}

/// An ordered set of [`MigrationAction`]s a [`Migrator`] would apply.
///
/// Tagged with the owning [`MigrationKind`]; paths are project-relative
/// throughout. `#[non_exhaustive]` so fields can grow.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct MigrationPlan {
    /// Migrator that produced the plan.
    pub kind: MigrationKind,
    /// Actions in apply order.
    pub actions: Vec<MigrationAction>,
}

impl MigrationPlan {
    /// Build a plan for `kind` from `actions`.
    #[must_use]
    pub const fn new(kind: MigrationKind, actions: Vec<MigrationAction>) -> Self {
        Self { kind, actions }
    }
}

/// Top-level outcome of an apply.
///
/// Structurally complete: an apply either committed at least one action
/// ([`MigrationStatus::Applied`]) or left the tree untouched
/// ([`MigrationStatus::Skipped`]); genuine failures surface as an
/// [`Error`], not a status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MigrationStatus {
    /// The plan was staged and committed into the live tree.
    Applied,
    /// Nothing to do — the plan was empty and the project is untouched.
    Skipped,
}

/// How a [`FileOutcome`] file came to be.
///
/// `#[non_exhaustive]` so it can track future [`MigrationAction`]
/// variants.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FileChange {
    /// File contents were replaced (or the file was created).
    Rewritten,
    /// File was relocated to a new project-relative path.
    Moved,
    /// File was deleted from the live tree.
    Removed,
}

/// Per-file result of an applied [`MigrationAction`].
///
/// Carries the resulting path, how it changed, and the sha256 of its
/// bytes. `#[non_exhaustive]` so fields can grow.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct FileOutcome {
    /// Project-relative path the file now lives at.
    pub path: PathBuf,
    /// Whether the file was rewritten or moved.
    pub change: FileChange,
    /// sha256 hex digest of the resulting file bytes.
    pub sha256: String,
    /// Origin path for a move; absent for a rewrite.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<PathBuf>,
}

/// Post-apply report mirroring the [`MigrationPlan`] shape.
///
/// Adds per-file checksums, a top-level [`MigrationStatus`], and the two
/// counts the `migration.applied` journal event needs.
/// `#[non_exhaustive]` so fields can grow.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct MigrationReport {
    /// Migrator that produced the report.
    pub kind: MigrationKind,
    /// Top-level outcome.
    pub status: MigrationStatus,
    /// Per-file outcomes in apply order.
    pub files: Vec<FileOutcome>,
    /// Count of files rewritten in place (journal `files-rewritten`).
    pub files_rewritten: usize,
    /// Count of files moved (journal `files-moved`).
    pub files_moved: usize,
}

/// A single resolved file operation awaiting staging + commit.
///
/// Every action is resolved to one of these before the harness touches
/// the live tree, so a precondition failure (missing move source or
/// removal target) aborts before any mutation.
enum Staged {
    /// A file to materialise into place — a rewrite (no origin) or a
    /// move (origin dropped after the commit rename).
    Write {
        /// Project-relative destination path.
        rel: PathBuf,
        /// Resulting file bytes.
        bytes: Vec<u8>,
        /// Outcome category for the report ([`FileChange::Rewritten`]
        /// or [`FileChange::Moved`]).
        change: FileChange,
        /// Move origin to drop after the commit rename; `None` for a
        /// rewrite.
        origin: Option<PathBuf>,
    },
    /// A file to delete from the live tree. Carries no staged bytes —
    /// the removal happens in the post-commit cleanup pass — and the
    /// sha256 of the bytes that were removed (read while resolving the
    /// existence precondition) for the report.
    Remove {
        /// Project-relative path to delete.
        rel: PathBuf,
        /// sha256 hex digest of the removed bytes.
        sha256: String,
    },
}

/// Shared staged-write-then-rename apply harness (RFC-30 §Atomicity).
///
/// Resolves every action first; a precondition failure (a missing move
/// source or removal target) aborts before any mutation, so the
/// project is left untouched. It then stages every rewritten / moved
/// file under `.specify/.migrate/<kind>/staging/`, renames each into
/// place, then runs a single cleanup pass that drops move sources
/// ([`MigrationAction::Move`]) and removal targets
/// ([`MigrationAction::Remove`]), and clears the staging tree. The
/// commit renames and the cleanup deletions are the one window in
/// which a crash could leave a partially-applied tree — the same
/// atomicity envelope the journal append accepts.
///
/// An empty plan is a no-op that returns [`MigrationStatus::Skipped`]
/// without touching the tree.
///
/// # Errors
///
/// - [`Error::Filesystem`] when a move source or removal target is
///   unreadable (`migrate-read-source`) or a staging / commit / cleanup
///   filesystem step fails (`migrate-stage` / `migrate-commit` /
///   `migrate-remove-source` / `migrate-remove`).
/// - [`Error::Io`] when creating the staging tree fails.
pub fn apply_staged(project_dir: &Path, plan: &MigrationPlan) -> Result<MigrationReport, Error> {
    if plan.actions.is_empty() {
        return Ok(MigrationReport {
            kind: plan.kind,
            status: MigrationStatus::Skipped,
            files: Vec::new(),
            files_rewritten: 0,
            files_moved: 0,
        });
    }

    let mut staged: Vec<Staged> = Vec::with_capacity(plan.actions.len());
    for action in &plan.actions {
        staged.push(resolve_action(project_dir, action)?);
    }

    let layout = Layout::new(project_dir);
    let staging = layout.migrate_staging_dir(plan.kind.id());
    reset_dir(&staging)?;
    for item in &staged {
        if let Staged::Write { rel, bytes, .. } = item {
            stage(&staging.join(rel), bytes)?;
        }
    }

    for item in &staged {
        if let Staged::Write { rel, .. } = item {
            commit(&staging.join(rel), &project_dir.join(rel))?;
        }
    }
    for item in &staged {
        match item {
            Staged::Write {
                rel,
                origin: Some(origin),
                ..
            } if origin != rel => {
                drop_file("migrate-remove-source", &project_dir.join(origin))?;
            }
            Staged::Remove { rel, .. } => {
                drop_file("migrate-remove", &project_dir.join(rel))?;
            }
            Staged::Write { .. } => {}
        }
    }

    let _ = std::fs::remove_dir_all(layout.migrate_dir(plan.kind.id())).ok();

    let files: Vec<FileOutcome> = staged
        .iter()
        .map(|item| match item {
            Staged::Write {
                rel,
                bytes,
                change,
                origin,
            } => FileOutcome {
                path: rel.clone(),
                change: *change,
                sha256: sha256_hex(bytes),
                from: origin.clone(),
            },
            Staged::Remove { rel, sha256 } => FileOutcome {
                path: rel.clone(),
                change: FileChange::Removed,
                sha256: sha256.clone(),
                from: None,
            },
        })
        .collect();
    let files_rewritten = files.iter().filter(|f| f.change == FileChange::Rewritten).count();
    let files_moved = files.iter().filter(|f| f.change == FileChange::Moved).count();

    Ok(MigrationReport {
        kind: plan.kind,
        status: MigrationStatus::Applied,
        files,
        files_rewritten,
        files_moved,
    })
}

/// Resolve one action to its destination + resulting bytes without
/// mutating the live tree. Reading a missing move source fails here, so
/// the caller can abort before any commit.
fn resolve_action(project_dir: &Path, action: &MigrationAction) -> Result<Staged, Error> {
    match action {
        MigrationAction::Rewrite { path, contents } => Ok(Staged::Write {
            rel: path.clone(),
            bytes: contents.clone().into_bytes(),
            change: FileChange::Rewritten,
            origin: None,
        }),
        MigrationAction::Move { from, to } => {
            let bytes = read_source(&project_dir.join(from))?;
            Ok(Staged::Write {
                rel: to.clone(),
                bytes,
                change: FileChange::Moved,
                origin: Some(from.clone()),
            })
        }
        MigrationAction::Remove { path } => {
            let bytes = read_source(&project_dir.join(path))?;
            Ok(Staged::Remove {
                rel: path.clone(),
                sha256: sha256_hex(&bytes),
            })
        }
    }
}

/// Read a precondition source file (a move origin or removal target),
/// mapping a missing / unreadable file to the `migrate-read-source`
/// [`Error::Filesystem`] so the caller aborts before any mutation.
fn read_source(path: &Path) -> Result<Vec<u8>, Error> {
    std::fs::read(path).map_err(|source| Error::Filesystem {
        op: "migrate-read-source",
        path: path.to_path_buf(),
        source,
    })
}

/// Delete a live file as part of the post-commit cleanup pass. Absent
/// files are tolerated (the precondition pass already proved the file
/// existed at resolve time; a concurrent disappearance is a no-op).
fn drop_file(op: &'static str, path: &Path) -> Result<(), Error> {
    if path.exists() {
        std::fs::remove_file(path).map_err(|source| Error::Filesystem {
            op,
            path: path.to_path_buf(),
            source,
        })?;
    }
    Ok(())
}

/// Reset `dir` to an empty directory, discarding any prior staging.
fn reset_dir(dir: &Path) -> Result<(), Error> {
    if dir.exists() {
        std::fs::remove_dir_all(dir)?;
    }
    std::fs::create_dir_all(dir)?;
    Ok(())
}

/// Write `bytes` to a staging path, creating parent directories.
fn stage(staged_path: &Path, bytes: &[u8]) -> Result<(), Error> {
    if let Some(parent) = staged_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(staged_path, bytes).map_err(|source| Error::Filesystem {
        op: "migrate-stage",
        path: staged_path.to_path_buf(),
        source,
    })
}

/// Rename a staged file into its live destination (atomic per file on a
/// shared filesystem; both paths live under the project root).
fn commit(staged_path: &Path, dest: &Path) -> Result<(), Error> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::rename(staged_path, dest).map_err(|source| Error::Filesystem {
        op: "migrate-commit",
        path: dest.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests;
