//! `specify migrate *` — one-shot movers for the on-disk layout
//! transitions that operator projects need to apply once when their
//! Specify CLI crosses the matching cut-over boundary.
//!
//! ## `specify migrate v2-layout` (RFC-9 §1B / RFC-13 chunk 2.0)
//!
//! Moves operator-facing platform artifacts from `.specify/` to the
//! repo root:
//!
//! - `.specify/registry.yaml` -> `registry.yaml`
//! - `.specify/plan.yaml`     -> `plan.yaml`
//! - `.specify/initiative.md` -> `initiative.md`
//! - `.specify/contracts/`    -> `contracts/`
//!
//! Idempotent. Refuses to clobber a non-empty destination — if both
//! the legacy and the v2 path already exist, the operator must
//! inspect manually and resolve (typically `rm` the empty
//! `.specify/...` half) before re-running. Refuses to run inside a
//! workspace clone (`.specify/workspace/<name>/`); the operator
//! migrates the hub repo first and then iterates clones explicitly.
//!
//! ## `specify migrate slice-layout` (RFC-13 chunk 3.6)
//!
//! Renames the per-loop-unit working directory from `.specify/changes/`
//! to `.specify/slices/`, and rewrites any in-tree `$CHANGE_DIR`
//! substitutions in vendored skill markdown to `$SLICE_DIR`.
//! Idempotent — a project already on the post-Phase-3 layout exits 0
//! with a no-op message. Refuses to run when any per-loop unit under
//! `.specify/changes/` carries a non-terminal lifecycle status
//! (`slice-migration-blocked-by-in-progress`); the operator must
//! finish (`specify slice merge run <name>`) or drop
//! (`specify slice drop <name>`) the in-progress slice first. Refuses
//! with `slice-migration-target-exists` when both `.specify/changes/`
//! and `.specify/slices/` are already on disk — a previous migration
//! was interrupted or someone hand-edited the tree, and the operator
//! must reconcile the two before re-running.
//!
//! Single-shot: this migration does not journal its own progress. If
//! interrupted mid-step the operator simply re-runs; the idempotency
//! guard above makes the second run safe.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use specify::{Error, ProjectConfig, SLICES_DIR_NAME, SliceMetadata};

use crate::cli::OutputFormat;
use crate::output::{CliResult, emit_response};

/// Pre-Phase-3 basename of the per-loop-unit working directory under
/// `.specify/`. RFC-13 chunk 3 renamed it to [`SLICES_DIR_NAME`]
/// ("slices"); this constant only survives so the slice-layout
/// migrator can name the v1 source directory in one place.
const LEGACY_CHANGES_DIR_NAME: &str = "changes";

/// Repo-relative legacy paths the migrator inspects, and the root-
/// relative destination each maps to. Returned in deterministic
/// order so JSON output is stable for fixture comparison.
const fn migrations() -> [Migration; 4] {
    [
        Migration {
            from: ".specify/registry.yaml",
            to: "registry.yaml",
            kind: ArtifactKind::File,
        },
        Migration {
            from: ".specify/plan.yaml",
            to: "plan.yaml",
            kind: ArtifactKind::File,
        },
        Migration {
            from: ".specify/initiative.md",
            to: "initiative.md",
            kind: ArtifactKind::File,
        },
        Migration {
            from: ".specify/contracts",
            to: "contracts",
            kind: ArtifactKind::Directory,
        },
    ]
}

#[derive(Debug, Clone, Copy)]
struct Migration {
    from: &'static str,
    to: &'static str,
    kind: ArtifactKind,
}

#[derive(Debug, Clone, Copy)]
enum ArtifactKind {
    File,
    Directory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum MoveStatus {
    /// Source moved to destination.
    Moved,
    /// Would move; printed in dry-run mode.
    WouldMove,
    /// Source absent — nothing to do.
    AbsentSource,
    /// Destination already exists; refused without writing anything.
    DestinationExists,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
struct MoveRow {
    from: String,
    to: String,
    status: MoveStatus,
}

/// Result envelope emitted by `specify migrate v2-layout`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
struct MigrateBody {
    moves: Vec<MoveRow>,
    /// `true` when at least one source needed migrating (whether
    /// performed or refused). `false` for the all-absent case.
    any_legacy_present: bool,
    /// `true` when at least one destination collision blocked the
    /// migration. The operator must resolve manually.
    any_collisions: bool,
    /// Echo of `--dry-run`.
    #[serde(skip_serializing_if = "Option::is_none")]
    dry_run: Option<bool>,
}

/// Run the v2-layout migration. Reads `project_dir` (typically the
/// CLI's CWD) for the four legacy paths and renames each to its v2
/// destination at the repo root.
///
/// Exit-code contract:
/// - `0` (`Success`): nothing to migrate, OR every present source moved.
/// - `1` (`GenericFailure`): at least one source/destination collision.
///
/// # Errors
///
/// Returns an error on filesystem failures other than the structured
/// `DestinationExists` outcome (e.g. a permission error during `rename`).
pub fn run_migrate_v2_layout(
    format: OutputFormat, project_dir: &Path, dry_run: bool,
) -> Result<CliResult, Error> {
    if is_inside_workspace_clone(project_dir) {
        return Err(Error::Config(format!(
            "specify migrate v2-layout: refusing to run inside a workspace clone at {}; \
             migrate the hub repo first, then iterate clones explicitly",
            project_dir.display()
        )));
    }

    let mut rows = Vec::with_capacity(4);
    let mut any_legacy_present = false;
    let mut any_collisions = false;

    for m in migrations() {
        let src = project_dir.join(m.from);
        let dst = project_dir.join(m.to);

        let src_exists = match m.kind {
            ArtifactKind::File => src.is_file(),
            ArtifactKind::Directory => src.is_dir(),
        };
        if !src_exists {
            rows.push(MoveRow {
                from: m.from.to_string(),
                to: m.to.to_string(),
                status: MoveStatus::AbsentSource,
            });
            continue;
        }
        any_legacy_present = true;

        let dst_exists = dst.exists();
        if dst_exists {
            any_collisions = true;
            rows.push(MoveRow {
                from: m.from.to_string(),
                to: m.to.to_string(),
                status: MoveStatus::DestinationExists,
            });
            continue;
        }

        if dry_run {
            rows.push(MoveRow {
                from: m.from.to_string(),
                to: m.to.to_string(),
                status: MoveStatus::WouldMove,
            });
            continue;
        }

        // Use the in-tree atomic helper for crash-safe rename when
        // available; for files / dirs that cross filesystems it falls
        // back to copy + remove. Bare `fs::rename` is fine inside the
        // same project root which is the common case.
        fs::rename(&src, &dst).map_err(|err| {
            Error::Io(std::io::Error::new(
                err.kind(),
                format!("specify migrate v2-layout: failed to move {} -> {}: {err}", m.from, m.to),
            ))
        })?;

        rows.push(MoveRow {
            from: m.from.to_string(),
            to: m.to.to_string(),
            status: MoveStatus::Moved,
        });
    }

    let body = MigrateBody {
        moves: rows,
        any_legacy_present,
        any_collisions,
        dry_run: dry_run.then_some(true),
    };

    match format {
        OutputFormat::Json => emit_response(&body),
        OutputFormat::Text => print_text(&body),
    }

    Ok(if any_collisions { CliResult::GenericFailure } else { CliResult::Success })
}

fn print_text(body: &MigrateBody) {
    if !body.any_legacy_present {
        println!("nothing to migrate (no legacy v1-layout artifacts found)");
        return;
    }
    if body.dry_run == Some(true) {
        println!("[dry-run] specify migrate v2-layout:");
    } else {
        println!("specify migrate v2-layout:");
    }
    for row in &body.moves {
        let label = match row.status {
            MoveStatus::Moved => "moved          ",
            MoveStatus::WouldMove => "would-move     ",
            MoveStatus::AbsentSource => "absent         ",
            MoveStatus::DestinationExists => "dest-exists    ",
        };
        println!("  {label}  {} -> {}", row.from, row.to);
    }
    if body.any_collisions {
        println!();
        println!("error: one or more destinations already exist; resolve manually then re-run");
    }
}

// ---------------------------------------------------------------------------
// `specify migrate slice-layout` (RFC-13 chunk 3.6)
// ---------------------------------------------------------------------------

/// Result envelope emitted by `specify migrate slice-layout`. Mirrors
/// the v2-layout shape (kebab-case keys, `dry-run` echoed) so JSON
/// consumers can branch on a uniform structure across migrations.
///
/// On-disk wire shape:
///
/// ```yaml
/// status: migrated | would-migrate | already-migrated | no-slices
/// slices-moved: <usize>           # equal to slice-names.len()
/// slice-names: [<name>, ...]      # alphabetical
/// skills-rewritten: [<rel>, ...]  # repo-relative, forward-slash
/// dry-run: true                   # only when --dry-run was passed
/// ```
#[derive(Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
struct SliceLayoutBody {
    /// Pre-state classification — see [`SliceLayoutStatus`].
    status: SliceLayoutStatus,
    /// Number of slice subdirectories moved (or that would move).
    /// Equal to `slice_names.len()`; surfaced as a top-level field so
    /// JSON consumers can branch without having to count the array.
    slices_moved: usize,
    /// Names of slice subdirectories under the renamed directory, in
    /// alphabetical order. Empty when the source was empty or the run
    /// was a no-op.
    slice_names: Vec<String>,
    /// Repo-relative paths (forward slashes) of vendored skill markdown
    /// files rewritten from `$CHANGE_DIR` to `$SLICE_DIR`. Empty when
    /// the project does not vendor skills locally.
    skills_rewritten: Vec<String>,
    /// Echo of `--dry-run`. Omitted when the run was not a dry run.
    #[serde(skip_serializing_if = "Option::is_none")]
    dry_run: Option<bool>,
}

/// Pre-state classification stamped on every slice-layout response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum SliceLayoutStatus {
    /// `.specify/changes/` was renamed to `.specify/slices/`.
    Migrated,
    /// `.specify/changes/` would be renamed (dry-run mode).
    WouldMigrate,
    /// `.specify/slices/` already exists and `.specify/changes/` does
    /// not — re-running on an already-migrated project.
    AlreadyMigrated,
    /// Neither directory exists — fresh project with no slices yet.
    NoSlices,
}

/// Run the slice-layout migration (RFC-13 chunk 3.6). Renames
/// `.specify/changes/` to `.specify/slices/` and rewrites any in-tree
/// `$CHANGE_DIR` substitutions in vendored skill markdown to
/// `$SLICE_DIR`.
///
/// Algorithm (single-shot, no journal):
///
/// 1. Confirm we're in a Specify project (`.specify/project.yaml`
///    must exist; otherwise [`Error::NotInitialized`]).
/// 2. Refuse to run inside a `.specify/workspace/<peer>/` clone (the
///    same guard the v2-layout migration applies).
/// 3. Inspect the two directories:
///    - Both absent: no-op (`no-slices`), exit 0.
///    - Source absent, destination present: no-op (`already-migrated`),
///      exit 0.
///    - Both present: refuse with [`Error::SliceMigrationTargetExists`].
///    - Source present, destination absent: continue.
/// 4. Walk every immediate child of `.specify/changes/`. For each
///    directory carrying a readable `.metadata.yaml`, classify by
///    [`specify::LifecycleStatus::is_terminal`]; collect the in-progress
///    offenders. Refuse with
///    [`Error::SliceMigrationBlockedByInProgress`] when the list is
///    non-empty.
/// 5. `fs::rename` the directory atomically (same-filesystem in the
///    common case; cross-FS callers can pre-stage manually).
/// 6. Walk the project's `plugins/` subtree (judgement-call vendoring
///    location — many projects don't have one) and rewrite literal
///    `$CHANGE_DIR` to `$SLICE_DIR` in every `*.md` file.
/// 7. Render the summary (text or JSON) and return [`CliResult::Success`].
///
/// Dry-run mode runs the same preflight (steps 1–4) so the operator
/// sees the same diagnostics they would in a real run, then reports
/// what *would* happen without writing anything to disk.
///
/// # Errors
///
/// - [`Error::NotInitialized`] when `.specify/project.yaml` is absent.
/// - [`Error::Config`] when invoked inside `.specify/workspace/<peer>/`.
/// - [`Error::SliceMigrationTargetExists`] when both the v1 source
///   and the post-migration destination are on disk.
/// - [`Error::SliceMigrationBlockedByInProgress`] when one or more
///   slices under `.specify/changes/` carry a non-terminal lifecycle
///   status.
/// - [`Error::Io`] on filesystem failures during the rename or skill
///   rewrite walk.
/// - [`Error::Yaml`] when a slice's `.metadata.yaml` cannot be parsed
///   to classify its lifecycle status (the operator must repair the
///   metadata before migrating).
pub fn run_migrate_slice_layout(
    format: OutputFormat, project_dir: &Path, dry_run: bool,
) -> Result<CliResult, Error> {
    if !ProjectConfig::config_path(project_dir).is_file() {
        return Err(Error::NotInitialized);
    }
    if is_inside_workspace_clone(project_dir) {
        return Err(Error::Config(format!(
            "specify migrate slice-layout: refusing to run inside a workspace clone at {}; \
             migrate the hub repo first, then iterate clones explicitly",
            project_dir.display()
        )));
    }

    let specify_dir = ProjectConfig::specify_dir(project_dir);
    let changes_dir = specify_dir.join(LEGACY_CHANGES_DIR_NAME);
    let slices_dir = specify_dir.join(SLICES_DIR_NAME);

    let changes_present = changes_dir.is_dir();
    let slices_present = slices_dir.is_dir();

    let body = match (changes_present, slices_present) {
        (false, true) => SliceLayoutBody {
            status: SliceLayoutStatus::AlreadyMigrated,
            slices_moved: 0,
            slice_names: Vec::new(),
            skills_rewritten: Vec::new(),
            dry_run: dry_run.then_some(true),
        },
        (false, false) => SliceLayoutBody {
            status: SliceLayoutStatus::NoSlices,
            slices_moved: 0,
            slice_names: Vec::new(),
            skills_rewritten: Vec::new(),
            dry_run: dry_run.then_some(true),
        },
        (true, true) => {
            return Err(Error::SliceMigrationTargetExists {
                changes: changes_dir,
                slices: slices_dir,
            });
        }
        (true, false) => {
            let in_progress = scan_in_progress(&changes_dir)?;
            if !in_progress.is_empty() {
                return Err(Error::SliceMigrationBlockedByInProgress { in_progress });
            }
            let slice_names = list_slice_names(&changes_dir)?;
            if !dry_run {
                fs::rename(&changes_dir, &slices_dir).map_err(|err| {
                    Error::Io(std::io::Error::new(
                        err.kind(),
                        format!(
                            "specify migrate slice-layout: failed to rename {} -> {}: {err}",
                            changes_dir.display(),
                            slices_dir.display()
                        ),
                    ))
                })?;
            }
            let skills_rewritten = rewrite_vendored_skills(project_dir, dry_run)?;
            SliceLayoutBody {
                status: if dry_run {
                    SliceLayoutStatus::WouldMigrate
                } else {
                    SliceLayoutStatus::Migrated
                },
                slices_moved: slice_names.len(),
                slice_names,
                skills_rewritten,
                dry_run: dry_run.then_some(true),
            }
        }
    };

    match format {
        OutputFormat::Json => emit_response(&body),
        OutputFormat::Text => print_slice_layout_text(&body),
    }
    Ok(CliResult::Success)
}

fn print_slice_layout_text(body: &SliceLayoutBody) {
    let prefix = if body.dry_run == Some(true) {
        "[dry-run] specify migrate slice-layout:"
    } else {
        "specify migrate slice-layout:"
    };
    match body.status {
        SliceLayoutStatus::AlreadyMigrated => {
            println!(
                "{prefix} nothing to migrate (already on the post-Phase-3 layout — \
                 `.specify/slices/` is in place)"
            );
        }
        SliceLayoutStatus::NoSlices => {
            println!(
                "{prefix} nothing to migrate (no `.specify/changes/` and no \
                 `.specify/slices/` on disk)"
            );
        }
        SliceLayoutStatus::WouldMigrate => {
            println!(
                "{prefix} would rename `.specify/changes/` -> `.specify/slices/` \
                 ({} slice(s)); would rewrite `$CHANGE_DIR` -> `$SLICE_DIR` in \
                 {} skill markdown file(s)",
                body.slices_moved,
                body.skills_rewritten.len(),
            );
            for name in &body.slice_names {
                println!("  slice    {name}");
            }
            for path in &body.skills_rewritten {
                println!("  rewrite  {path}");
            }
        }
        SliceLayoutStatus::Migrated => {
            println!(
                "{prefix} renamed `.specify/changes/` -> `.specify/slices/` ({} slice(s)); \
                 rewrote `$CHANGE_DIR` -> `$SLICE_DIR` in {} skill markdown file(s). \
                 migration complete.",
                body.slices_moved,
                body.skills_rewritten.len(),
            );
            for name in &body.slice_names {
                println!("  slice    {name}");
            }
            for path in &body.skills_rewritten {
                println!("  rewrite  {path}");
            }
        }
    }
}

/// Walk every immediate child of `<changes_dir>` (the v1 source). For
/// each directory carrying a `.metadata.yaml`, parse it and record
/// the `(name, status)` pair when the status is non-terminal.
///
/// Subdirectories without a `.metadata.yaml` are quietly skipped — a
/// half-created slice carries no lifecycle invariant we'd corrupt by
/// renaming it. A malformed `.metadata.yaml` propagates as
/// [`Error::Yaml`] so the operator repairs the offending file before
/// re-running; refusing to migrate on bad metadata is safer than
/// silently moving an unclassifiable slice.
///
/// The returned vector is sorted by slice name so the diagnostic and
/// the JSON output stay stable across runs.
fn scan_in_progress(changes_dir: &Path) -> Result<Vec<(String, String)>, Error> {
    let mut offenders: Vec<(String, String)> = Vec::new();
    for entry in fs::read_dir(changes_dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let dir = entry.path();
        let Some(name) = dir.file_name().and_then(|s| s.to_str()).map(str::to_string) else {
            continue;
        };
        if !SliceMetadata::path(&dir).is_file() {
            continue;
        }
        let metadata = SliceMetadata::load(&dir)?;
        if !metadata.status.is_terminal() {
            offenders.push((name, metadata.status.to_string()));
        }
    }
    offenders.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(offenders)
}

/// Enumerate the slice subdirectory basenames under `<changes_dir>`
/// in alphabetical order. Used purely for the post-migration summary;
/// the rename moves the directory wholesale so we never iterate to
/// rename anything by name.
///
/// Skips entries that aren't directories (and any stray top-level
/// files an operator might have dropped under `.specify/changes/`).
fn list_slice_names(changes_dir: &Path) -> Result<Vec<String>, Error> {
    let mut names: Vec<String> = Vec::new();
    for entry in fs::read_dir(changes_dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        if let Some(name) = entry.file_name().to_str().map(str::to_string) {
            names.push(name);
        }
    }
    names.sort();
    Ok(names)
}

/// Walk `<project_dir>/plugins/` recursively and rewrite literal
/// `$CHANGE_DIR` to `$SLICE_DIR` in every `*.md` file. Returns the
/// repo-relative paths (forward slashes) of files that needed the
/// rewrite, in sorted order.
///
/// Returns an empty vector when `plugins/` is absent — many projects
/// don't vendor skills locally. `.specify/.cache/`, `.specify/archive/`,
/// `.specify/workspace/`, and the canonical scratch dirs (`target/`,
/// `node_modules/`, dotfile dirs) are not touched: cached upstream
/// briefs refresh on the next `specify capability resolve` and the
/// rest is build/scratch state.
///
/// Under `dry_run` no file is written; the returned list still
/// reflects what *would* be rewritten so the summary is accurate.
fn rewrite_vendored_skills(project_dir: &Path, dry_run: bool) -> Result<Vec<String>, Error> {
    let plugins_dir = project_dir.join("plugins");
    if !plugins_dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut rewritten: Vec<String> = Vec::new();
    walk_and_rewrite(&plugins_dir, project_dir, dry_run, &mut rewritten)?;
    rewritten.sort();
    Ok(rewritten)
}

/// Recursive worker for [`rewrite_vendored_skills`]. Visits every
/// regular `*.md` file under `dir`; substitutes `$CHANGE_DIR` ->
/// `$SLICE_DIR` when the literal is present; appends the repo-
/// relative path (forward slashes) to `rewritten` for the summary.
///
/// Skips dotfile directories (e.g. `.git`, `.cache`) so a
/// `plugins/.foo/` scratch dir doesn't pull us off course.
fn walk_and_rewrite(
    dir: &Path, project_dir: &Path, dry_run: bool, rewritten: &mut Vec<String>,
) -> Result<(), Error> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            walk_and_rewrite(&path, project_dir, dry_run, rewritten)?;
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        if path.extension().and_then(|s| s.to_str()) != Some("md") {
            continue;
        }
        let content = fs::read_to_string(&path)?;
        if !content.contains("$CHANGE_DIR") {
            continue;
        }
        let updated = content.replace("$CHANGE_DIR", "$SLICE_DIR");
        if !dry_run {
            fs::write(&path, updated)?;
        }
        let rel = path.strip_prefix(project_dir).unwrap_or(&path);
        rewritten.push(rel.to_string_lossy().replace('\\', "/"));
    }
    Ok(())
}

/// Detect whether `project_dir` sits *inside* a `.specify/workspace/<name>/`
/// path so the migrator refuses to touch peer clones. Conservative:
/// only true when the chain `.../foo/.specify/workspace/bar/...`
/// appears literally in the path components.
fn is_inside_workspace_clone(project_dir: &Path) -> bool {
    // Canonicalise best-effort; if the path doesn't exist yet (the CWD
    // always does in practice), fall back to the input path.
    let path = fs::canonicalize(project_dir).unwrap_or_else(|_| PathBuf::from(project_dir));
    let parts: Vec<&std::ffi::OsStr> =
        path.components().map(std::path::Component::as_os_str).collect();
    parts.windows(3).any(|w| {
        w[0] == std::ffi::OsStr::new(".specify")
            && w[1] == std::ffi::OsStr::new("workspace")
            && !w[2].is_empty()
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    fn legacy_layout(dir: &Path) {
        let specify = dir.join(".specify");
        fs::create_dir_all(&specify).unwrap();
        fs::write(specify.join("registry.yaml"), "version: 1\nprojects: []\n").unwrap();
        fs::write(specify.join("plan.yaml"), "name: x\nchanges: []\n").unwrap();
        fs::write(specify.join("initiative.md"), "---\nname: x\n---\n").unwrap();
        let contracts = specify.join("contracts").join("schemas");
        fs::create_dir_all(&contracts).unwrap();
        fs::write(contracts.join("payload.yaml"), "type: object\n").unwrap();
    }

    #[test]
    fn migrate_moves_all_four_artifacts() {
        let tmp = tempdir().unwrap();
        legacy_layout(tmp.path());

        let result = run_migrate_v2_layout(OutputFormat::Json, tmp.path(), false).unwrap();
        assert_eq!(result, CliResult::Success);

        // v2 destinations exist at the root.
        assert!(tmp.path().join("registry.yaml").is_file());
        assert!(tmp.path().join("plan.yaml").is_file());
        assert!(tmp.path().join("initiative.md").is_file());
        assert!(tmp.path().join("contracts").join("schemas").join("payload.yaml").is_file());

        // v1 sources are gone.
        assert!(!tmp.path().join(".specify/registry.yaml").exists());
        assert!(!tmp.path().join(".specify/plan.yaml").exists());
        assert!(!tmp.path().join(".specify/initiative.md").exists());
        assert!(!tmp.path().join(".specify/contracts").exists());
    }

    #[test]
    fn migrate_is_idempotent() {
        let tmp = tempdir().unwrap();
        legacy_layout(tmp.path());

        let r1 = run_migrate_v2_layout(OutputFormat::Json, tmp.path(), false).unwrap();
        let r2 = run_migrate_v2_layout(OutputFormat::Json, tmp.path(), false).unwrap();
        assert_eq!(r1, CliResult::Success);
        assert_eq!(r2, CliResult::Success, "second run on already-migrated tree must succeed");
    }

    #[test]
    fn migrate_dry_run_writes_nothing() {
        let tmp = tempdir().unwrap();
        legacy_layout(tmp.path());

        let result = run_migrate_v2_layout(OutputFormat::Json, tmp.path(), true).unwrap();
        assert_eq!(result, CliResult::Success);

        // v1 sources still present, v2 destinations not yet.
        assert!(tmp.path().join(".specify/registry.yaml").is_file());
        assert!(tmp.path().join(".specify/contracts").is_dir());
        assert!(!tmp.path().join("registry.yaml").exists());
        assert!(!tmp.path().join("contracts").exists());
    }

    #[test]
    fn migrate_refuses_destination_collision() {
        let tmp = tempdir().unwrap();
        legacy_layout(tmp.path());
        // Plant a colliding root-level file.
        fs::write(tmp.path().join("registry.yaml"), "pre-existing\n").unwrap();

        let result = run_migrate_v2_layout(OutputFormat::Json, tmp.path(), false).unwrap();
        assert_eq!(result, CliResult::GenericFailure, "collision must surface as exit 1");

        // Source must remain untouched.
        assert!(tmp.path().join(".specify/registry.yaml").is_file());
        let pre = fs::read_to_string(tmp.path().join("registry.yaml")).expect("read pre-existing");
        assert_eq!(pre, "pre-existing\n", "pre-existing destination must not be clobbered");
    }

    #[test]
    fn migrate_no_op_when_nothing_to_do() {
        let tmp = tempdir().unwrap();
        // No legacy layout, no v2 layout — clean tempdir.
        let result = run_migrate_v2_layout(OutputFormat::Json, tmp.path(), false).unwrap();
        assert_eq!(result, CliResult::Success);
    }

    // --- slice-layout (RFC-13 chunk 3.6) ------------------------------------

    /// Seed the bare-minimum on-disk shape that satisfies the
    /// migrator's "is this a Specify project?" preflight. The migration
    /// itself never reads `project.yaml`'s contents.
    fn seed_specify_project(dir: &Path) {
        let specify = dir.join(".specify");
        fs::create_dir_all(&specify).unwrap();
        fs::write(specify.join("project.yaml"), "name: demo\ncapability: omnia\n").unwrap();
    }

    /// Write a minimal `.metadata.yaml` for a slice in a given
    /// lifecycle status. The status string must match
    /// [`specify::LifecycleStatus`]'s lowercase serde discriminant.
    fn write_slice_metadata_at(slice_dir: &Path, status: &str) {
        fs::create_dir_all(slice_dir).unwrap();
        fs::write(
            slice_dir.join(".metadata.yaml"),
            format!("schema: omnia\nstatus: {status}\n"),
        )
        .unwrap();
    }

    #[test]
    fn slice_layout_renames_changes_and_preserves_contents() {
        let tmp = tempdir().unwrap();
        seed_specify_project(tmp.path());
        let changes = tmp.path().join(".specify").join("changes");
        write_slice_metadata_at(&changes.join("alpha"), "merged");
        write_slice_metadata_at(&changes.join("beta"), "dropped");
        // Stash a non-metadata payload to confirm the rename is a
        // wholesale move, not an iterate-and-copy.
        fs::write(changes.join("alpha").join("notes.md"), "# alpha\n").unwrap();

        let result =
            run_migrate_slice_layout(OutputFormat::Json, tmp.path(), false).unwrap();
        assert_eq!(result, CliResult::Success);

        // Source directory is gone.
        assert!(!tmp.path().join(".specify/changes").exists());
        // Destination has both slices with their original payload.
        assert!(tmp.path().join(".specify/slices/alpha/.metadata.yaml").is_file());
        assert!(tmp.path().join(".specify/slices/beta/.metadata.yaml").is_file());
        let payload = fs::read_to_string(tmp.path().join(".specify/slices/alpha/notes.md"))
            .expect("alpha payload");
        assert_eq!(payload, "# alpha\n");
    }

    #[test]
    fn slice_layout_is_idempotent_when_already_migrated() {
        let tmp = tempdir().unwrap();
        seed_specify_project(tmp.path());
        // Pre-create the destination to simulate a project that has
        // already run the migration once.
        fs::create_dir_all(tmp.path().join(".specify/slices/gamma")).unwrap();

        let result =
            run_migrate_slice_layout(OutputFormat::Json, tmp.path(), false).unwrap();
        assert_eq!(result, CliResult::Success, "re-run on post-Phase-3 layout must succeed");
    }

    #[test]
    fn slice_layout_no_slices_anywhere_is_no_op() {
        let tmp = tempdir().unwrap();
        seed_specify_project(tmp.path());

        let result =
            run_migrate_slice_layout(OutputFormat::Json, tmp.path(), false).unwrap();
        assert_eq!(result, CliResult::Success);
    }

    #[test]
    fn slice_layout_blocks_on_in_progress_slice() {
        let tmp = tempdir().unwrap();
        seed_specify_project(tmp.path());
        let changes = tmp.path().join(".specify").join("changes");
        write_slice_metadata_at(&changes.join("alpha"), "merged");
        // `defining` is the canonical first non-terminal status.
        write_slice_metadata_at(&changes.join("zeta"), "defining");

        let err = run_migrate_slice_layout(OutputFormat::Json, tmp.path(), false)
            .expect_err("non-terminal slice must block");
        match err {
            Error::SliceMigrationBlockedByInProgress { in_progress } => {
                assert_eq!(in_progress.len(), 1);
                assert_eq!(in_progress[0].0, "zeta");
                assert_eq!(in_progress[0].1, "defining");
            }
            other => panic!("wrong error: {other:?}"),
        }
        // Source still on disk so the operator can finish or drop.
        assert!(tmp.path().join(".specify/changes/zeta").is_dir());
        assert!(!tmp.path().join(".specify/slices").exists());
    }

    #[test]
    fn slice_layout_refuses_when_both_directories_present() {
        let tmp = tempdir().unwrap();
        seed_specify_project(tmp.path());
        write_slice_metadata_at(&tmp.path().join(".specify/changes/alpha"), "merged");
        fs::create_dir_all(tmp.path().join(".specify/slices/already-here")).unwrap();

        let err = run_migrate_slice_layout(OutputFormat::Json, tmp.path(), false)
            .expect_err("both dirs present must refuse");
        assert!(matches!(err, Error::SliceMigrationTargetExists { .. }));
        // Neither side should be modified.
        assert!(tmp.path().join(".specify/changes/alpha").is_dir());
        assert!(tmp.path().join(".specify/slices/already-here").is_dir());
    }

    #[test]
    fn slice_layout_dry_run_writes_nothing() {
        let tmp = tempdir().unwrap();
        seed_specify_project(tmp.path());
        write_slice_metadata_at(&tmp.path().join(".specify/changes/alpha"), "merged");

        let result =
            run_migrate_slice_layout(OutputFormat::Json, tmp.path(), true).unwrap();
        assert_eq!(result, CliResult::Success);
        assert!(tmp.path().join(".specify/changes/alpha").is_dir());
        assert!(!tmp.path().join(".specify/slices").exists());
    }

    #[test]
    fn slice_layout_rewrites_vendored_skill_markdown() {
        let tmp = tempdir().unwrap();
        seed_specify_project(tmp.path());
        write_slice_metadata_at(&tmp.path().join(".specify/changes/alpha"), "merged");
        let skill_dir = tmp.path().join("plugins/spec/skills/define");
        fs::create_dir_all(&skill_dir).unwrap();
        let skill_path = skill_dir.join("SKILL.md");
        fs::write(&skill_path, "Read $CHANGE_DIR/proposal.md, write $CHANGE_DIR/spec.md.\n")
            .unwrap();
        // A non-markdown file with $CHANGE_DIR must NOT be rewritten —
        // the migration is scoped to skill markdown only.
        let helper_path = skill_dir.join("helper.txt");
        fs::write(&helper_path, "$CHANGE_DIR\n").unwrap();
        // A `.specify/.cache/` file must NOT be rewritten (it lives
        // outside `plugins/`, which is the only walk root).
        let cache_dir = tmp.path().join(".specify/.cache/omnia/briefs");
        fs::create_dir_all(&cache_dir).unwrap();
        let cache_path = cache_dir.join("proposal.md");
        fs::write(&cache_path, "$CHANGE_DIR cache\n").unwrap();

        let result =
            run_migrate_slice_layout(OutputFormat::Json, tmp.path(), false).unwrap();
        assert_eq!(result, CliResult::Success);

        let rewritten = fs::read_to_string(&skill_path).unwrap();
        assert!(rewritten.contains("$SLICE_DIR/proposal.md"), "got: {rewritten}");
        assert!(!rewritten.contains("$CHANGE_DIR"), "got: {rewritten}");
        // Helper text and cached briefs must be untouched.
        assert_eq!(fs::read_to_string(&helper_path).unwrap(), "$CHANGE_DIR\n");
        assert_eq!(fs::read_to_string(&cache_path).unwrap(), "$CHANGE_DIR cache\n");
    }

    #[test]
    fn slice_layout_requires_specify_project() {
        let tmp = tempdir().unwrap();
        // No `.specify/project.yaml` at all.
        let err = run_migrate_slice_layout(OutputFormat::Json, tmp.path(), false)
            .expect_err("missing project must refuse");
        assert!(matches!(err, Error::NotInitialized));
    }
}
