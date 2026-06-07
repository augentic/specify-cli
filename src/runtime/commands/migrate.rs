use std::io::Write;
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use serde::Serialize;
use specify_error::{Error, Result};
use specify_model::atomic::yaml_write;
use specify_workflow::config::{Layout, ProjectConfig};
use specify_workflow::journal::{self, Event, EventKind};
use specify_workflow::migrate::{
    self, FileOutcome, MigrationAction, MigrationKind, MigrationPlan, MigrationReport,
    MigrationStatus, migrator_for,
};

use crate::runtime::cli::Format;
use crate::runtime::output;

/// Bootstrap migration command. Resolves config through the migration
/// carve-out ([`ProjectConfig::load_for_migration`]), walks the
/// registered hop chain from `from` to `to`, and either previews
/// (`--dry-run`) or applies (`--yes`) each migrator's plan.
pub(super) fn run(
    format: Format, from: Option<&str>, to: Option<&str>, dry_run: bool, yes: bool,
) -> Result<()> {
    let project_dir = PathBuf::from(".");
    let (mut config, _migration) = ProjectConfig::load_for_migration(&project_dir)?;

    let from_str = match from {
        Some(value) => value.to_string(),
        None => config.specify_version.clone().ok_or_else(|| Error::Diag {
            code: "migrate-from-unknown",
            detail: "no --from given and project.yaml pins no specify_version to migrate from"
                .to_string(),
        })?,
    };
    let to_str = to.map_or_else(|| env!("CARGO_PKG_VERSION").to_string(), str::to_string);

    let from_major = migrate::major(&from_str).ok_or_else(|| unparseable(&from_str))?;
    let to_major = migrate::major(&to_str).ok_or_else(|| unparseable(&to_str))?;

    let kinds = MigrationKind::resolve(from_major, to_major);
    if kinds.is_empty() {
        return emit(format, &MigrateBody::none(from_str, to_str, dry_run));
    }

    if dry_run {
        let outcomes = kinds
            .iter()
            .map(|&kind| planned_outcome(&project_dir, kind))
            .collect::<Result<Vec<_>>>()?;
        return emit(format, &MigrateBody::new(from_str, to_str, true, outcomes));
    }

    if !yes {
        return Err(Error::Diag {
            code: "migrate-consent-required",
            detail: "refusing to apply a migration without consent; pass --yes to apply or \
                     --dry-run to preview"
                .to_string(),
        });
    }

    let layout = Layout::new(&project_dir);
    let mut outcomes = Vec::with_capacity(kinds.len());
    for kind in &kinds {
        let migrator = migrator_for(*kind);
        let plan = migrator.plan(&project_dir)?;
        match migrator.apply(&project_dir, &plan) {
            Ok(report) => {
                if report.status == MigrationStatus::Applied {
                    let event = Event::new(
                        Timestamp::now(),
                        EventKind::MigrationApplied {
                            kind: report.kind.id().to_string(),
                            files_rewritten: report.files_rewritten,
                            files_moved: report.files_moved,
                        },
                    );
                    journal::append_batch(layout, std::slice::from_ref(&event))?;
                }
                outcomes.push(KindOutcome::applied(&plan, &report));
            }
            Err(err) => {
                let event = Event::new(
                    Timestamp::now(),
                    EventKind::MigrationSkipped {
                        kind: kind.id().to_string(),
                        reason: err.variant_str().into_owned(),
                    },
                );
                drop(journal::append_batch(layout, std::slice::from_ref(&event)));
                return Err(err);
            }
        }
    }

    config.specify_version = Some(to_str.clone());
    yaml_write(&layout.config_path(), &config)?;

    emit(format, &MigrateBody::new(from_str, to_str, false, outcomes))
}

/// `migrate-version-unparseable` for a `--from` / `--to` value that is
/// not a semantic version.
fn unparseable(value: &str) -> Error {
    Error::Diag {
        code: "migrate-version-unparseable",
        detail: format!("could not parse `{value}` as a semantic version"),
    }
}

/// Build the dry-run [`KindOutcome`] for `kind` from its pure plan.
fn planned_outcome(project_dir: &Path, kind: MigrationKind) -> Result<KindOutcome> {
    let plan = migrator_for(kind).plan(project_dir)?;
    Ok(KindOutcome::planned(kind, &plan))
}

/// Wire-stable `specify migrate` envelope (text + JSON).
#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct MigrateBody {
    /// Resolved source version the walk started from.
    from: String,
    /// Resolved target version the walk ended at.
    to: String,
    /// `true` for `--dry-run`; `false` on the apply path.
    dry_run: bool,
    /// `true` when at least one kind carried (or would carry) actions.
    migrated: bool,
    /// Per-kind plan / report rows in walk order.
    kinds: Vec<KindOutcome>,
}

impl MigrateBody {
    /// Envelope for a window with no registered migrators.
    const fn none(from: String, to: String, dry_run: bool) -> Self {
        Self {
            from,
            to,
            dry_run,
            migrated: false,
            kinds: Vec::new(),
        }
    }

    /// Envelope carrying `kinds`; `migrated` is `true` when any kind
    /// has actions.
    fn new(from: String, to: String, dry_run: bool, kinds: Vec<KindOutcome>) -> Self {
        let migrated = kinds.iter().any(|kind| !kind.actions.is_empty());
        Self {
            from,
            to,
            dry_run,
            migrated,
            kinds,
        }
    }
}

/// One migrator's outcome on the `specify migrate` envelope.
#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct KindOutcome {
    /// Stable migrator id (e.g. `v2-to-v3`).
    kind: String,
    /// `planned` (dry-run), `applied`, or `skipped` (empty plan).
    status: &'static str,
    /// Files rewritten in place — the `migration.applied` count.
    files_rewritten: usize,
    /// Files moved — the `migration.applied` count.
    files_moved: usize,
    /// The plan actions in apply order.
    actions: Vec<MigrationAction>,
    /// Per-file outcomes; populated only on the apply path.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    files: Vec<FileOutcome>,
}

impl KindOutcome {
    /// Dry-run row: derive the would-fire counts from the plan.
    fn planned(kind: MigrationKind, plan: &MigrationPlan) -> Self {
        Self {
            kind: kind.id().to_string(),
            status: "planned",
            files_rewritten: count(plan, |a| matches!(a, MigrationAction::Rewrite { .. })),
            files_moved: count(plan, |a| matches!(a, MigrationAction::Move { .. })),
            actions: plan.actions.clone(),
            files: Vec::new(),
        }
    }

    /// Apply row: carry the report's counts and per-file outcomes.
    fn applied(plan: &MigrationPlan, report: &MigrationReport) -> Self {
        let status = match report.status {
            MigrationStatus::Applied => "applied",
            MigrationStatus::Skipped => "skipped",
        };
        Self {
            kind: report.kind.id().to_string(),
            status,
            files_rewritten: report.files_rewritten,
            files_moved: report.files_moved,
            actions: plan.actions.clone(),
            files: report.files.clone(),
        }
    }
}

/// Count plan actions matching `pred`.
fn count(plan: &MigrationPlan, pred: impl Fn(&MigrationAction) -> bool) -> usize {
    plan.actions.iter().filter(|action| pred(action)).count()
}

fn write_text(w: &mut dyn Write, body: &MigrateBody) -> std::io::Result<()> {
    if body.kinds.is_empty() {
        writeln!(
            w,
            "No migration needed: {} -> {} has no registered migrators.",
            body.from, body.to
        )?;
        return Ok(());
    }
    let verb = if body.dry_run { "Planned" } else { "Applied" };
    writeln!(w, "{verb} migration {} -> {}", body.from, body.to)?;
    for kind in &body.kinds {
        writeln!(
            w,
            "  {} ({}): {} rewritten, {} moved",
            kind.kind, kind.status, kind.files_rewritten, kind.files_moved
        )?;
        for action in &kind.actions {
            match action {
                MigrationAction::Rewrite { path, .. } => {
                    writeln!(w, "    rewrite {}", path.display())?;
                }
                MigrationAction::Move { from, to } => {
                    writeln!(w, "    move {} -> {}", from.display(), to.display())?;
                }
                MigrationAction::Remove { path } => {
                    writeln!(w, "    remove {}", path.display())?;
                }
                _ => {}
            }
        }
        if body.dry_run {
            writeln!(
                w,
                "    would emit migration.applied {{ kind: {}, files-rewritten: {}, files-moved: {} }}",
                kind.kind, kind.files_rewritten, kind.files_moved
            )?;
        }
    }
    Ok(())
}

fn emit(format: Format, body: &MigrateBody) -> Result<()> {
    output::emit(&mut std::io::stdout().lock(), format, body, write_text)?;
    Ok(())
}
