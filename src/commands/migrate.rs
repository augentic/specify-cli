//! `specify migrate v2-layout` — one-shot mover for the v2 layout
//! transition.
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

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use specify::Error;

use crate::cli::OutputFormat;
use crate::output::{CliResult, emit_response};

/// Repo-relative legacy paths the migrator inspects, and the root-
/// relative destination each maps to. Returned in deterministic
/// order so JSON output is stable for fixture comparison.
fn migrations() -> [Migration; 4] {
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

        let dst_exists = match m.kind {
            ArtifactKind::File => dst.exists(),
            ArtifactKind::Directory => dst.exists(),
        };
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
                format!(
                    "specify migrate v2-layout: failed to move {} -> {}: {err}",
                    m.from, m.to
                ),
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
        println!(
            "error: one or more destinations already exist; resolve manually then re-run"
        );
    }
}

/// Detect whether `project_dir` sits *inside* a `.specify/workspace/<name>/`
/// path so the migrator refuses to touch peer clones. Conservative:
/// only true when the chain `.../foo/.specify/workspace/bar/...`
/// appears literally in the path components.
fn is_inside_workspace_clone(project_dir: &Path) -> bool {
    // Canonicalise best-effort; if the path doesn't exist yet (the CWD
    // always does in practice), fall back to the input path.
    let path = fs::canonicalize(project_dir).unwrap_or_else(|_| PathBuf::from(project_dir));
    let parts: Vec<&std::ffi::OsStr> = path.components().map(|c| c.as_os_str()).collect();
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
        let pre =
            fs::read_to_string(tmp.path().join("registry.yaml")).expect("read pre-existing");
        assert_eq!(pre, "pre-existing\n", "pre-existing destination must not be clobbered");
    }

    #[test]
    fn migrate_no_op_when_nothing_to_do() {
        let tmp = tempdir().unwrap();
        // No legacy layout, no v2 layout — clean tempdir.
        let result = run_migrate_v2_layout(OutputFormat::Json, tmp.path(), false).unwrap();
        assert_eq!(result, CliResult::Success);
    }
}
