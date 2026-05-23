//! `specify workspace *` handlers — `sync`, `prepare`, `push`.

pub mod cli;

use std::io::Write;
use std::path::PathBuf;

use serde::Serialize;
use specify_domain::change::Plan;
use specify_domain::registry::Registry;
use specify_domain::registry::branch::{Prepared, Request as BranchRequest, prepare as prepare_branch};
use specify_domain::registry::workspace::{PushOutcome, PushResult, push_projects, sync_projects};
use specify_error::{Error, Result};

use crate::context::Ctx;

pub fn sync(ctx: &Ctx, projects: &[String]) -> Result<()> {
    let registry = match Registry::load(&ctx.project_dir)? {
        None if !projects.is_empty() => return Err(registry_missing()),
        other => other,
    };
    let synced = if let Some(reg) = registry.as_ref() {
        let selected = reg.select(projects)?;
        sync_projects(&ctx.project_dir, &selected)?;
        true
    } else {
        false
    };
    let message = (!synced).then_some("no registry declared at registry.yaml; nothing to sync");
    ctx.write(
        &SyncBody {
            registry,
            synced,
            message,
        },
        write_sync_text,
    )?;
    Ok(())
}

pub fn prepare(
    ctx: &Ctx, project: &str, change: String, sources: Vec<PathBuf>, outputs: Vec<PathBuf>,
) -> Result<()> {
    let Some(registry) = Registry::load(&ctx.project_dir)? else {
        return Err(registry_missing());
    };
    let project_filter = [project.to_string()];
    let selected = registry.select(&project_filter)?;
    let Some(project) = selected.first() else {
        return Err(Error::Diag {
            code: "workspace-prepare-no-project",
            detail: "workspace prepare resolved no project".to_string(),
        });
    };
    let request = BranchRequest {
        change_name: change,
        source_paths: sources,
        output_paths: outputs,
    };

    match prepare_branch(&ctx.project_dir, project, &request) {
        Ok(prepared) => {
            ctx.write(
                &PrepareBody {
                    prepared: true,
                    inner: &prepared,
                },
                write_prepare_text,
            )?;
            Ok(())
        }
        Err(diagnostic) => Err(Error::BranchPrepareFailed {
            project: project.name.clone(),
            key: diagnostic.key,
            detail: diagnostic.message,
            paths: diagnostic.paths,
        }),
    }
}

pub fn push(ctx: &Ctx, projects: &[String], dry_run: bool) -> Result<()> {
    let Some(registry) = Registry::load(&ctx.project_dir)? else {
        return Err(registry_missing());
    };
    let selected = registry.select(projects)?;

    let plan_path = ctx.layout().plan_path();
    if !plan_path.exists() {
        return Err(Error::Diag {
            code: "workspace-push-no-plan",
            detail: "No active plan found at plan.yaml. Run `/spec:plan <name>` (or \
                     `specify plan create <name>`) to scaffold a fresh plan, or check whether \
                     the plan was already archived."
                .to_string(),
        });
    }
    let plan = Plan::load(&plan_path)?;

    let results = push_projects(&ctx.project_dir, &plan.name, &selected, dry_run)?;
    let any_failed = results.iter().any(|r| r.status == PushOutcome::Failed);

    let plan_name = plan.name.clone();
    ctx.write(
        &PushBody {
            plan_name: plan.name,
            projects: results,
            dry_run,
        },
        write_push_text,
    )?;

    if any_failed {
        Err(Error::Diag {
            code: "workspace-push-failed",
            detail: format!(
                "workspace push for plan `{plan_name}` had at least one failed project; \
                 see the stdout body for per-project status"
            ),
        })
    } else {
        Ok(())
    }
}

fn registry_missing() -> Error {
    Error::Diag {
        code: "registry-missing",
        detail: "no registry declared at registry.yaml".to_string(),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct SyncBody {
    registry: Option<Registry>,
    synced: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<&'static str>,
}

fn write_sync_text(w: &mut dyn Write, body: &SyncBody) -> std::io::Result<()> {
    if body.synced {
        writeln!(w, "workspace sync complete")
    } else {
        writeln!(w, "no registry declared at registry.yaml; nothing to sync")
    }
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct PrepareBody<'a> {
    prepared: bool,
    #[serde(flatten)]
    inner: &'a Prepared,
}

fn write_prepare_text(w: &mut dyn Write, body: &PrepareBody<'_>) -> std::io::Result<()> {
    let p = body.inner;
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

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct PushBody {
    #[serde(skip)]
    plan_name: String,
    projects: Vec<PushResult>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    dry_run: bool,
}

fn write_push_text(w: &mut dyn Write, body: &PushBody) -> std::io::Result<()> {
    let prefix = if body.dry_run { "[dry-run] " } else { "" };
    writeln!(w, "{prefix}specify: workspace push — {}", body.plan_name)?;
    writeln!(w)?;
    let mut counts = [0_usize; 6];
    for r in &body.projects {
        let raw = r.status.to_string();
        let label =
            if body.dry_run && matches!(r.status, PushOutcome::Pushed | PushOutcome::Created) {
                format!("would-{raw}")
            } else {
                raw
            };
        let pr = r.pr_number.map_or_else(String::new, |n| format!("PR #{n}"));
        writeln!(w, "  {:<20} {:<14} {} {}", r.name, label, r.branch.as_deref().unwrap_or(""), pr)?;
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
