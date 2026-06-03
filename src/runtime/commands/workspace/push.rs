use std::io::Write;

use serde::Serialize;
use specify_error::{Error, Result};
use specify_workflow::change::Plan;
use specify_workflow::registry::Registry;
use specify_workflow::registry::workspace::{PushOutcome, PushResult, push_projects};

use super::registry_missing;
use crate::runtime::context::Ctx;

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
            plan_name: plan.name.to_string(),
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
