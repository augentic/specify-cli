pub mod cli;

use std::io::Write;

use jiff::Timestamp;
use serde::Serialize;
use specify_domain::capability::ChangeBrief;
use specify_domain::change::finalize;
use specify_domain::cmd::RealCmd;
use specify_domain::registry::Registry;
use specify_domain::slice::atomic::bytes_write;
use specify_error::{Error, Result};

use self::cli::ChangeAction;
use crate::cli::SourceArg;
use crate::commands::plan;
use crate::context::Ctx;

/// Dispatch `specify change *` — operator brief and finalize.
pub fn run(ctx: &Ctx, action: ChangeAction) -> Result<()> {
    match action {
        ChangeAction::Create { name, sources } => create(ctx, name, sources),
        ChangeAction::Show => brief_show(ctx),
        ChangeAction::Finalize { clean, dry_run } => run_finalize(ctx, clean, dry_run),
    }
}

/// Scaffold both `change.md` and `plan.yaml` atomically.
///
/// Atomicity contract: if either file already exists, refuse with
/// `already-exists` and write neither. Validation order is fixed —
/// kebab-case first, source-argument shape next, file collisions last
/// — so operators see the most actionable diagnostic first. The plan
/// half delegates to [`plan::write_scaffold`], the same helper that
/// backs `specify plan create`.
fn create(ctx: &Ctx, name: String, sources: Vec<SourceArg>) -> Result<()> {
    plan::require_kebab_change_name(&name)?;
    let source_map = plan::build_source_map(sources)?;

    let brief_path = ChangeBrief::path(&ctx.project_dir);
    let plan_path = ctx.layout().plan_path();
    let mut existing: Vec<String> = Vec::new();
    if brief_path.exists() {
        existing.push(format!("change brief at {}", brief_path.display()));
    }
    if plan_path.exists() {
        existing.push(format!("plan at {}", plan_path.display()));
    }
    if !existing.is_empty() {
        return Err(Error::Diag {
            code: "already-exists",
            detail: format!("refusing to overwrite existing {}", existing.join(" and ")),
        });
    }

    bytes_write(&brief_path, ChangeBrief::template(&name).as_bytes())?;
    plan::write_scaffold(&plan_path, &name, source_map)?;

    ctx.write(
        &CreateBody {
            name,
            brief: brief_path.display().to_string(),
            plan: plan_path.display().to_string(),
        },
        write_create_text,
    )?;
    Ok(())
}

fn brief_show(ctx: &Ctx) -> Result<()> {
    let brief_path = ChangeBrief::path(&ctx.project_dir);
    let brief = ChangeBrief::load(&ctx.project_dir)?;
    let path = brief_path.display().to_string();
    ctx.write(&brief, |w, brief| match brief {
        None => writeln!(w, "no change brief declared at {path}"),
        Some(brief) => render_brief(w, brief, &path),
    })?;
    Ok(())
}

// ---------------------------------------------------------------------------
// `specify change finalize`
// ---------------------------------------------------------------------------

fn run_finalize(ctx: &Ctx, clean: bool, dry_run: bool) -> Result<()> {
    let plan = match finalize::load_plan(&ctx.project_dir)? {
        finalize::PlanLoad::Present(plan) => plan,
        finalize::PlanLoad::Missing => {
            return Err(Error::Diag {
                code: "plan-not-found",
                detail: "no plan to finalize: plan.yaml is absent. \
                         If the change was already finalized, the archive is at \
                         .specify/archive/plans/. Otherwise run \
                         `specify change create <name> [--source ...]` to scaffold \
                         change.md and plan.yaml together and start the loop."
                    .to_string(),
            });
        }
    };

    // Registry is optional — an empty registry means no per-project
    // probes to run, but the archive (and the `--clean` no-op) still
    // make sense.
    let registry = Registry::load(&ctx.project_dir)?.unwrap_or(Registry {
        version: 1,
        projects: Vec::new(),
    });

    let inputs = finalize::Inputs {
        project_dir: &ctx.project_dir,
        plan: &plan,
        registry: &registry,
        clean,
        dry_run,
        now: Timestamp::now(),
    };

    match finalize::run(inputs, &RealCmd) {
        Ok(outcome) => {
            let finalized = outcome.finalized;
            let summary = blocked_reason(&outcome.summary);
            let plan_name = outcome.name.clone();
            ctx.write(&outcome, render_finalize_outcome)?;
            if finalized {
                Ok(())
            } else {
                Err(Error::Diag {
                    code: "change-finalize-blocked",
                    detail: format!("change `{plan_name}` blocked: {summary}"),
                })
            }
        }
        Err(finalize::Refusal::NonTerminalEntries(entries)) => Err(Error::Diag {
            code: "non-terminal-entries-present",
            detail: format!("plan `{}` has non-terminal entries: {entries:?}", plan.name),
        }),
    }
}

// ---------------------------------------------------------------------------
// DTOs
// ---------------------------------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct CreateBody {
    name: String,
    brief: String,
    plan: String,
}

fn write_create_text(w: &mut dyn Write, body: &CreateBody) -> std::io::Result<()> {
    writeln!(w, "Created change brief for {} at {}", body.name, body.brief)?;
    writeln!(w, "Initialised plan '{}' at {}.", body.name, body.plan)
}

fn render_brief(w: &mut dyn Write, brief: &ChangeBrief, path: &str) -> std::io::Result<()> {
    writeln!(w, "change brief: {path}")?;
    writeln!(w, "name: {}", brief.frontmatter.name)?;
    if brief.frontmatter.inputs.is_empty() {
        writeln!(w, "inputs: (none)")?;
    } else {
        writeln!(w, "inputs:")?;
        for input in &brief.frontmatter.inputs {
            writeln!(w, "  - path: {}", input.path)?;
            writeln!(w, "    kind: {}", input.kind)?;
        }
    }
    writeln!(w)?;
    write!(w, "{}", brief.body)
}

/// Text-format rendering for [`finalize::Outcome`]. Used by
/// [`Ctx::emit_with`] in [`run_finalize`] — the domain type ships its
/// own `Serialize`, so the binary only needs to own the text shape.
fn render_finalize_outcome(w: &mut dyn Write, outcome: &finalize::Outcome) -> std::io::Result<()> {
    let prefix = if outcome.dry_run { "[dry-run] " } else { "" };
    writeln!(
        w,
        "{prefix}specify: change finalize \u{2014} {} ({})",
        outcome.name, outcome.expected_branch
    )?;
    writeln!(w)?;

    for r in &outcome.projects {
        render_project_row(w, r)?;
    }
    if !outcome.projects.is_empty() {
        writeln!(w)?;
    }
    render_summary_line(w, &outcome.summary)?;
    writeln!(w)?;

    if outcome.finalized {
        if outcome.dry_run {
            writeln!(w, "[dry-run] Change `{}` would be finalized.", outcome.name)?;
        } else {
            writeln!(w, "Change `{}` finalized.", outcome.name)?;
            if let Some(archived) = &outcome.archived {
                writeln!(w, "  archived plan: {archived}")?;
            }
            if let Some(dir) = &outcome.archived_plans_dir {
                writeln!(w, "  archived dir:  {dir}")?;
            }
            if !outcome.cleaned.is_empty() {
                writeln!(w, "  cleaned clones: {}", outcome.cleaned.join(", "))?;
            }
        }
    } else {
        let reason = blocked_reason(&outcome.summary);
        writeln!(w, "Change `{}` blocked: {reason}.", outcome.name)?;
        if let Some(message) = &outcome.message {
            writeln!(w, "{message}")?;
        }
    }
    Ok(())
}

fn render_project_row(w: &mut dyn Write, r: &finalize::ProjectResult) -> std::io::Result<()> {
    let pr = r.pr_number.map(|n| format!("PR #{n}")).unwrap_or_default();
    let url = r.url.as_deref().unwrap_or("");
    writeln!(w, "  {:<20} {:<24} {:<10} {}", r.name, r.status, pr, url)?;
    if let Some(detail) = &r.detail {
        writeln!(w, "    {detail}")?;
    }
    Ok(())
}

fn render_summary_line(w: &mut dyn Write, s: &finalize::Summary) -> std::io::Result<()> {
    writeln!(
        w,
        "{} merged, {} unmerged, {} closed, {} no-branch, {} branch-pattern-mismatch, \
         {} dirty, {} failed.",
        s.merged, s.unmerged, s.closed, s.no_branch, s.branch_pattern_mismatch, s.dirty, s.failed,
    )
}

fn blocked_reason(s: &finalize::Summary) -> String {
    let mut reasons: Vec<String> = Vec::new();
    if s.unmerged > 0 {
        reasons.push(format!("{} unmerged PR(s) awaiting operator merge", s.unmerged));
    }
    if s.closed > 0 {
        reasons.push(format!("{} closed PR(s)", s.closed));
    }
    if s.branch_pattern_mismatch > 0 {
        reasons.push(format!("{} branch-pattern mismatch(es)", s.branch_pattern_mismatch));
    }
    if s.dirty > 0 {
        reasons.push(format!("{} dirty workspace clone(s)", s.dirty));
    }
    if s.failed > 0 {
        reasons.push(format!("{} probe failure(s)", s.failed));
    }
    if reasons.is_empty() { "see per-project status above".to_string() } else { reasons.join(", ") }
}

// ---------------------------------------------------------------------------
// Tests for the CLI handler — keep them lean; the heavy lifting lives in
// `specify_domain::change::finalize` (orchestrator) and `tests/cli.rs`
// (end-to-end with the real binary).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use specify_domain::change::finalize::Landing;

    use super::*;

    #[test]
    fn passing_statuses_only_merged_and_no_branch() {
        for s in [Landing::Merged, Landing::NoBranch] {
            assert!(s.is_passing(), "{s} must pass");
        }
        for s in [
            Landing::Unmerged,
            Landing::Closed,
            Landing::BranchPatternMismatch,
            Landing::Dirty,
            Landing::Failed,
        ] {
            assert!(!s.is_passing(), "{s} must refuse");
        }
    }

    #[test]
    fn blocked_reason_points_unmerged_prs_at_operator_merge() {
        let summary = finalize::Summary {
            unmerged: 2,
            ..finalize::Summary::default()
        };
        let reason = blocked_reason(&summary);
        assert!(
            reason.contains("operator merge"),
            "unmerged blocked reason must mention operator merge, got: {reason}",
        );
    }
}
