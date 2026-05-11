//! `specify workspace *` handlers — `sync`, `status`, `prepare-branch`, `push`.

pub(crate) mod cli;

use std::fmt;
use std::io::Write;
use std::path::PathBuf;

use serde::{Serialize, Serializer};
use specify_change::Plan;
use specify_config::LayoutExt;
use specify_error::{Error, Result};
use specify_registry::Registry;
use specify_registry::branch::{
    Diagnostic as BranchDiagnostic, Prepared, Request as BranchRequest, prepare,
};
use specify_registry::workspace::{
    ConfiguredTargetKind, PushOutcome, SlotKind, SlotStatus, push_projects,
    status_projects as workspace_status_projects, sync_projects as workspace_sync_projects,
};

use crate::context::Ctx;
use crate::output::{CliResult, Render, Stream, emit};

pub(crate) fn sync(ctx: &Ctx, projects: Vec<String>) -> Result<()> {
    let registry = match Registry::load(&ctx.project_dir)? {
        None if !projects.is_empty() => return Err(Error::RegistryMissing),
        other => other,
    };
    let synced = if let Some(reg) = registry.as_ref() {
        let selected = reg.select(&projects)?;
        workspace_sync_projects(&ctx.project_dir, &selected)?;
        true
    } else {
        false
    };
    let message = (!synced).then_some("no registry declared at registry.yaml; nothing to sync");
    emit(
        Stream::Stdout,
        ctx.format,
        &SyncBody {
            registry,
            synced,
            message,
        },
    )?;
    Ok(())
}

pub(crate) fn status(ctx: &Ctx, projects: Vec<String>) -> Result<()> {
    let body = match Registry::load(&ctx.project_dir)? {
        None => {
            if !projects.is_empty() {
                return Err(Error::RegistryMissing);
            }
            StatusBody::Absent {
                registry: None,
                slots: None,
            }
        }
        Some(registry) => {
            let selected = registry.select(&projects)?;
            let slots = workspace_status_projects(&ctx.project_dir, &selected)
                .iter()
                .map(SlotRow::from)
                .collect();
            StatusBody::Present { slots }
        }
    };
    emit(Stream::Stdout, ctx.format, &body)?;
    Ok(())
}

pub(crate) fn prepare_branch(
    ctx: &Ctx, project: String, change: String, sources: Vec<PathBuf>, outputs: Vec<PathBuf>,
) -> Result<CliResult> {
    let Some(registry) = Registry::load(&ctx.project_dir)? else {
        return Err(Error::RegistryMissing);
    };
    let selected = registry.select(std::slice::from_ref(&project))?;
    let Some(project) = selected.first() else {
        return Err(Error::Diag {
            code: "workspace-prepare-branch-no-project",
            detail: "workspace prepare-branch resolved no project".to_string(),
        });
    };
    let request = BranchRequest {
        change_name: change,
        source_paths: sources,
        output_paths: outputs,
    };

    match prepare(&ctx.project_dir, project, &request) {
        Ok(prepared) => {
            emit(
                Stream::Stdout,
                ctx.format,
                &PrepareBranchBody {
                    prepared: true,
                    inner: &prepared,
                    diagnostics: Vec::new(),
                },
            )?;
            Ok(CliResult::Success)
        }
        Err(diagnostic) => {
            let exit = CliResult::GenericFailure;
            emit(
                Stream::Stdout,
                ctx.format,
                &PrepareBranchErrBody {
                    error: "branch-preparation-failed",
                    exit_code: exit.code(),
                    diagnostic: &diagnostic,
                },
            )?;
            Ok(exit)
        }
    }
}

pub(crate) fn push(ctx: &Ctx, projects: Vec<String>, dry_run: bool) -> Result<CliResult> {
    let Some(registry) = Registry::load(&ctx.project_dir)? else {
        return Err(Error::RegistryMissing);
    };
    let selected = registry.select(&projects)?;

    let plan_path = ctx.project_dir.layout().plan_path();
    if !plan_path.exists() {
        return Err(Error::Diag {
            code: "workspace-push-no-plan",
            detail: "No active plan found at plan.yaml. Run 'specify change plan create' \
                     to create one, or check whether the plan was already archived."
                .to_string(),
        });
    }
    let plan = Plan::load(&plan_path)?;

    let results = push_projects(&ctx.project_dir, &plan.name, &selected, dry_run)?;
    let any_failed = results.iter().any(|r| r.status == PushOutcome::Failed);
    let items: Vec<PushItem> = results
        .iter()
        .map(|r| PushItem {
            name: r.name.clone(),
            status: r.status,
            branch: r.branch.clone(),
            pr: r.pr_number,
            error: r.error.clone(),
        })
        .collect();

    emit(
        Stream::Stdout,
        ctx.format,
        &PushBody {
            plan_name: plan.name,
            dry_run_flag: dry_run,
            projects: items,
            dry_run: dry_run.then_some(true),
        },
    )?;

    Ok(if any_failed { CliResult::GenericFailure } else { CliResult::Success })
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct SyncBody {
    registry: Option<Registry>,
    synced: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<&'static str>,
}

impl Render for SyncBody {
    fn render_text(&self, w: &mut dyn Write) -> std::io::Result<()> {
        if self.synced {
            writeln!(w, "workspace sync complete")
        } else {
            writeln!(w, "no registry declared at registry.yaml; nothing to sync")
        }
    }
}

#[derive(Serialize)]
#[serde(untagged, rename_all = "kebab-case")]
enum StatusBody {
    Absent { registry: Option<Registry>, slots: Option<Vec<SlotRow>> },
    Present { slots: Vec<SlotRow> },
}

impl Render for StatusBody {
    fn render_text(&self, w: &mut dyn Write) -> std::io::Result<()> {
        match self {
            Self::Absent { .. } => writeln!(w, "no registry declared at registry.yaml"),
            Self::Present { slots } => {
                for slot in slots {
                    slot.render_line(w)?;
                }
                Ok(())
            }
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct SlotRow {
    name: String,
    kind: SlotKind,
    slot_path: String,
    configured_target_kind: ConfiguredTargetKind,
    configured_target: String,
    actual_symlink_target: Option<String>,
    actual_origin: Option<String>,
    current_branch: Option<String>,
    head_sha: Option<String>,
    dirty: Option<bool>,
    branch_matches_change: Option<bool>,
    project_config_present: bool,
    active_slices: Vec<String>,
}

impl From<&SlotStatus> for SlotRow {
    fn from(slot: &SlotStatus) -> Self {
        Self {
            name: slot.name.clone(),
            kind: slot.kind,
            slot_path: slot.slot_path.display().to_string(),
            configured_target_kind: slot.configured_target_kind,
            configured_target: slot.configured_target.clone(),
            actual_symlink_target: slot
                .actual_symlink_target
                .as_ref()
                .map(|p| p.display().to_string()),
            actual_origin: slot.actual_origin.clone(),
            current_branch: slot.current_branch.clone(),
            head_sha: slot.head_sha.clone(),
            dirty: slot.dirty,
            branch_matches_change: slot.branch_matches_change,
            project_config_present: slot.project_config_present,
            active_slices: slot.active_slices.clone(),
        }
    }
}

impl SlotRow {
    fn render_line(&self, w: &mut dyn Write) -> std::io::Result<()> {
        writeln!(
            w,
            "{}: kind={} path={} configured-{}={} target={} origin={} branch={} change-branch={} head={} dirty={} project.yaml={} active-slices={}",
            self.name,
            self.kind,
            self.slot_path,
            self.configured_target_kind,
            self.configured_target,
            self.actual_symlink_target.as_deref().unwrap_or("-"),
            self.actual_origin.as_deref().unwrap_or("-"),
            self.current_branch.as_deref().unwrap_or("-"),
            MatchState::from(self.branch_matches_change),
            self.head_sha.as_deref().unwrap_or("-"),
            self.dirty.map_or("-", |v| if v { "yes" } else { "no" }),
            if self.project_config_present { "present" } else { "missing" },
            if self.active_slices.is_empty() {
                "-".to_string()
            } else {
                self.active_slices.join(",")
            },
        )
    }
}

/// Tri-state for `branch_matches_change` in the human-readable
/// status row: present-and-true is `match`, present-and-false is
/// `mismatch`, absent is `-`. JSON keeps the raw `Option<bool>` —
/// this only governs text rendering.
enum MatchState {
    Match,
    Mismatch,
    Unknown,
}

impl From<Option<bool>> for MatchState {
    fn from(v: Option<bool>) -> Self {
        match v {
            Some(true) => Self::Match,
            Some(false) => Self::Mismatch,
            None => Self::Unknown,
        }
    }
}

impl fmt::Display for MatchState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Match => "match",
            Self::Mismatch => "mismatch",
            Self::Unknown => "-",
        })
    }
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct PrepareBranchBody<'a> {
    prepared: bool,
    #[serde(flatten)]
    inner: &'a Prepared,
    diagnostics: Vec<BranchDiagnostic>,
}

impl Render for PrepareBranchBody<'_> {
    fn render_text(&self, w: &mut dyn Write) -> std::io::Result<()> {
        let p = self.inner;
        writeln!(
            w,
            "workspace branch prepared: {} {} ({:?}, {:?})",
            p.project, p.branch, p.local_branch, p.remote_branch
        )?;
        if !p.dirty.tracked_allowed.is_empty() || !p.dirty.untracked.is_empty() {
            writeln!(
                w,
                "dirty: {} tracked resume-safe, {} untracked",
                p.dirty.tracked_allowed.len(),
                p.dirty.untracked.len()
            )?;
        }
        Ok(())
    }
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct PrepareBranchErrBody<'a> {
    error: &'static str,
    exit_code: u8,
    diagnostic: &'a BranchDiagnostic,
}

impl Render for PrepareBranchErrBody<'_> {
    fn render_text(&self, w: &mut dyn Write) -> std::io::Result<()> {
        writeln!(w, "error: {}", self.diagnostic.message)?;
        writeln!(w, "diagnostic: {}", self.diagnostic.key)?;
        if !self.diagnostic.paths.is_empty() {
            writeln!(w, "paths: {}", self.diagnostic.paths.join(", "))?;
        }
        Ok(())
    }
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct PushBody {
    #[serde(skip)]
    plan_name: String,
    #[serde(skip)]
    dry_run_flag: bool,
    projects: Vec<PushItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    dry_run: Option<bool>,
}

impl Render for PushBody {
    fn render_text(&self, w: &mut dyn Write) -> std::io::Result<()> {
        let prefix = if self.dry_run_flag { "[dry-run] " } else { "" };
        writeln!(w, "{prefix}specify: workspace push — {}", self.plan_name)?;
        writeln!(w)?;
        let mut counts = [0usize; 6];
        for r in &self.projects {
            let raw = r.status.to_string();
            let label = if self.dry_run_flag
                && matches!(r.status, PushOutcome::Pushed | PushOutcome::Created)
            {
                format!("would-{raw}")
            } else {
                raw
            };
            let pr = r.pr.map_or_else(String::new, |n| format!("PR #{n}"));
            writeln!(
                w,
                "  {:<20} {:<14} {} {}",
                r.name,
                label,
                r.branch.as_deref().unwrap_or(""),
                pr
            )?;
            counts[match r.status {
                PushOutcome::Created => 0,
                PushOutcome::Pushed => 1,
                PushOutcome::UpToDate => 2,
                PushOutcome::LocalOnly => 3,
                PushOutcome::NoBranch => 4,
                PushOutcome::Failed => 5,
            }] += 1;
        }
        writeln!(w)?;
        writeln!(
            w,
            "{} created, {} pushed, {} up-to-date, {} local-only, {} no-branch. {} failed.",
            counts[0], counts[1], counts[2], counts[3], counts[4], counts[5]
        )
    }
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct PushItem {
    name: String,
    #[serde(serialize_with = "serialize_push_outcome")]
    status: PushOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pr: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde::serialize_with requires the `fn(&T, S) -> _` shape."
)]
fn serialize_push_outcome<S: Serializer>(o: &PushOutcome, s: S) -> Result<S::Ok, S::Error> {
    s.collect_str(o)
}
