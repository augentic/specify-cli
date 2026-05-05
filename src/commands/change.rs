#![allow(clippy::items_after_statements, clippy::option_if_let_else, clippy::unnecessary_wraps)]

pub mod plan;

use std::path::Path;

use serde::Serialize;
use serde_json::Value;
use specify::{ChangeBrief, Error, is_valid_kebab_name};
use specify_change::{
    FinalizeError, FinalizeInputs, FinalizeOutcome, FinalizeProjectResult, FinalizeSummaryCounts,
    RealFinalizeProbe, load_plan_or_refuse, run_finalize,
};
use specify_registry::Registry;

use crate::cli::{ChangeAction, OutputFormat};
use crate::context::CommandContext;
use crate::output::{CliResult, absolute_string, emit_response};

/// Dispatch `specify change *` (RFC-13 §"What becomes a capability").
///
/// `change` is the umbrella orchestration verb family — operator
/// brief, executable plan, and finalize. The `Plan { action }`
/// arm threads through to the plan submodule so the durable surface
/// reads `specify change plan {add,amend,next,status,...}`.
pub fn run_change(ctx: &CommandContext, action: ChangeAction) -> Result<CliResult, Error> {
    match action {
        ChangeAction::Create { name } => brief_create(ctx, name),
        ChangeAction::Show => brief_show(ctx),
        ChangeAction::Plan { action } => plan::run_plan(ctx, action),
        ChangeAction::Finalize { clean, dry_run } => finalize(ctx, clean, dry_run),
    }
}

fn brief_create(ctx: &CommandContext, name: String) -> Result<CliResult, Error> {
    if !is_valid_kebab_name(&name) {
        return Err(Error::Config(format!(
            "change brief: name `{name}` must be kebab-case \
             (lowercase ascii, digits, single hyphens; no leading/trailing/doubled hyphens)"
        )));
    }

    let brief_path = ChangeBrief::path(&ctx.project_dir);
    if brief_path.exists() {
        match ctx.format {
            OutputFormat::Json => {
                #[derive(Serialize)]
                #[serde(rename_all = "kebab-case")]
                struct BriefCreateErr {
                    action: &'static str,
                    ok: bool,
                    error: &'static str,
                    path: String,
                    exit_code: u8,
                }
                emit_response(BriefCreateErr {
                    action: "init",
                    ok: false,
                    error: "already-exists",
                    path: brief_path.display().to_string(),
                    exit_code: CliResult::GenericFailure.code(),
                });
            }
            OutputFormat::Text => {
                eprintln!(
                    "change brief already exists at {}; refusing to overwrite",
                    brief_path.display()
                );
            }
        }
        return Ok(CliResult::GenericFailure);
    }

    if let Some(parent) = brief_path.parent() {
        std::fs::create_dir_all(parent).map_err(Error::Io)?;
    }
    let rendered = ChangeBrief::template(&name);
    std::fs::write(&brief_path, &rendered).map_err(Error::Io)?;

    #[derive(Serialize)]
    #[serde(rename_all = "kebab-case")]
    struct BriefCreateOk {
        action: &'static str,
        ok: bool,
        name: String,
        path: String,
    }
    match ctx.format {
        OutputFormat::Json => emit_response(BriefCreateOk {
            action: "init",
            ok: true,
            name,
            path: absolute_string(&brief_path),
        }),
        OutputFormat::Text => {
            println!("Created change brief for {name} at {}", brief_path.display());
        }
    }
    Ok(CliResult::Success)
}

fn brief_show(ctx: &CommandContext) -> Result<CliResult, Error> {
    let brief_path = ChangeBrief::path(&ctx.project_dir);
    match ChangeBrief::load(&ctx.project_dir)? {
        None => {
            #[derive(Serialize)]
            #[serde(rename_all = "kebab-case")]
            struct BriefAbsent {
                brief: Value,
                path: String,
            }
            match ctx.format {
                OutputFormat::Json => emit_response(BriefAbsent {
                    brief: Value::Null,
                    path: brief_path.display().to_string(),
                }),
                OutputFormat::Text => {
                    println!(
                        "no change brief declared at {}",
                        brief_path.display()
                    );
                }
            }
            Ok(CliResult::Success)
        }
        Some(brief) => {
            #[derive(Serialize)]
            #[serde(rename_all = "kebab-case")]
            struct BriefBody {
                brief: BriefJson,
                path: String,
            }
            #[derive(Serialize)]
            #[serde(rename_all = "kebab-case")]
            struct BriefJson {
                frontmatter: specify::ChangeFrontmatter,
                body: String,
            }
            match ctx.format {
                OutputFormat::Json => emit_response(BriefBody {
                    brief: BriefJson {
                        frontmatter: brief.frontmatter.clone(),
                        body: brief.body,
                    },
                    path: brief_path.display().to_string(),
                }),
                OutputFormat::Text => print_change_brief_text(&brief, &brief_path),
            }
            Ok(CliResult::Success)
        }
    }
}

fn print_change_brief_text(brief: &ChangeBrief, brief_path: &Path) {
    println!("change brief: {}", brief_path.display());
    println!("name: {}", brief.frontmatter.name);
    if brief.frontmatter.inputs.is_empty() {
        println!("inputs: (none)");
    } else {
        println!("inputs:");
        for input in &brief.frontmatter.inputs {
            let kind = match input.kind {
                specify::InputKind::LegacyCode => "legacy-code",
                specify::InputKind::Documentation => "documentation",
            };
            println!("  - path: {}", input.path);
            println!("    kind: {kind}");
        }
    }
    println!();
    print!("{}", brief.body);
}

// ---------------------------------------------------------------------------
// `specify change finalize` (RFC-9 §4C)
// ---------------------------------------------------------------------------

fn finalize(ctx: &CommandContext, clean: bool, dry_run: bool) -> Result<CliResult, Error> {
    let plan_or_refusal = load_plan_or_refuse(&ctx.project_dir)?;
    let plan = match plan_or_refusal {
        Ok(plan) => plan,
        Err(FinalizeError::PlanNotFound) => {
            return Ok(emit_plan_not_found(ctx.format));
        }
        Err(FinalizeError::NonTerminalEntries(_)) => {
            unreachable!("load_plan_or_refuse only emits PlanNotFound");
        }
    };

    // Registry is optional — an empty registry means no per-project
    // probes to run, but the archive (and the `--clean` no-op) still
    // make sense. RFC-9 §4C does not require a registry.
    let registry = Registry::load(&ctx.project_dir)?.unwrap_or(Registry {
        version: 1,
        projects: Vec::new(),
    });

    let probe = RealFinalizeProbe;
    let inputs = FinalizeInputs {
        project_dir: &ctx.project_dir,
        plan: &plan,
        registry: &registry,
        clean,
        dry_run,
    };

    match run_finalize(inputs, &probe) {
        Ok(outcome) => {
            emit_outcome(ctx.format, &outcome);
            Ok(if outcome.finalized { CliResult::Success } else { CliResult::GenericFailure })
        }
        Err(FinalizeError::NonTerminalEntries(entries)) => {
            Ok(emit_non_terminal(ctx.format, &plan.name, &entries))
        }
        Err(FinalizeError::PlanNotFound) => {
            unreachable!("PlanNotFound is handled by load_plan_or_refuse")
        }
    }
}

fn emit_plan_not_found(format: OutputFormat) -> CliResult {
    let msg = "no plan to finalize: plan.yaml is absent. \
               If the change was already finalized, the archive is at \
               .specify/archive/plans/. Otherwise run \
               `specify change plan create` (and `specify change create` \
               if the change brief is also missing) to start the loop.";
    match format {
        OutputFormat::Json => {
            #[derive(Serialize)]
            #[serde(rename_all = "kebab-case")]
            struct PlanNotFound {
                error: &'static str,
                message: String,
                exit_code: u8,
            }
            emit_response(PlanNotFound {
                error: "plan-not-found",
                message: msg.to_string(),
                exit_code: CliResult::GenericFailure.code(),
            });
        }
        OutputFormat::Text => {
            eprintln!("error: {msg}");
        }
    }
    CliResult::GenericFailure
}

fn emit_non_terminal(format: OutputFormat, change: &str, entries: &[String]) -> CliResult {
    let entry_list = entries.join(", ");
    let msg = format!(
        "non-terminal-entries-present: plan `{change}` has {} entry(ies) not in a terminal \
         state: {entry_list}. Resolve them (transition done/failed/skipped) and re-run; see \
         `specify change plan status` for the full picture.",
        entries.len(),
    );
    match format {
        OutputFormat::Json => {
            #[derive(Serialize)]
            #[serde(rename_all = "kebab-case")]
            struct NonTerminal<'a> {
                error: &'static str,
                initiative: &'a str,
                entries: &'a [String],
                message: String,
                exit_code: u8,
            }
            emit_response(NonTerminal {
                error: "non-terminal-entries-present",
                initiative: change,
                entries,
                message: msg,
                exit_code: CliResult::GenericFailure.code(),
            });
        }
        OutputFormat::Text => {
            eprintln!("error: {msg}");
        }
    }
    CliResult::GenericFailure
}

fn emit_outcome(format: OutputFormat, outcome: &FinalizeOutcome) {
    match format {
        OutputFormat::Json => emit_response(outcome),
        OutputFormat::Text => print_outcome_text(outcome),
    }
}

fn print_outcome_text(outcome: &FinalizeOutcome) {
    if outcome.dry_run == Some(true) {
        println!(
            "[dry-run] specify: change finalize — {} ({})",
            outcome.initiative, outcome.expected_branch
        );
    } else {
        println!(
            "specify: change finalize — {} ({})",
            outcome.initiative, outcome.expected_branch
        );
    }
    println!();

    for r in &outcome.projects {
        print_project_row(r);
    }
    if !outcome.projects.is_empty() {
        println!();
    }

    print_summary_line(&outcome.summary);

    println!();
    if outcome.finalized {
        if outcome.dry_run == Some(true) {
            println!("[dry-run] Change `{}` would be finalized.", outcome.initiative);
        } else {
            println!("Change `{}` finalized.", outcome.initiative);
            if let Some(archived) = &outcome.archived {
                println!("  archived plan: {archived}");
            }
            if let Some(dir) = &outcome.archived_plans_dir {
                println!("  archived dir:  {dir}");
            }
            if !outcome.cleaned.is_empty() {
                println!("  cleaned clones: {}", outcome.cleaned.join(", "));
            }
        }
    } else {
        let reason = blocked_reason(&outcome.summary);
        println!("Change `{}` blocked: {reason}.", outcome.initiative);
    }
}

fn print_project_row(r: &FinalizeProjectResult) {
    let pr = r.pr_number.map(|n| format!("PR #{n}")).unwrap_or_default();
    let url = r.url.as_deref().unwrap_or("");
    println!("  {:<20} {:<24} {:<10} {}", r.name, r.status, pr, url);
    if let Some(detail) = &r.detail {
        println!("    {detail}");
    }
}

fn print_summary_line(s: &FinalizeSummaryCounts) {
    println!(
        "{} merged, {} unmerged, {} closed, {} no-branch, {} branch-pattern-mismatch, \
         {} dirty, {} failed.",
        s.merged, s.unmerged, s.closed, s.no_branch, s.branch_pattern_mismatch, s.dirty, s.failed,
    );
}

fn blocked_reason(s: &FinalizeSummaryCounts) -> String {
    let mut reasons: Vec<String> = Vec::new();
    if s.unmerged > 0 {
        reasons.push(format!("{} unmerged PR(s)", s.unmerged));
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
// `specify_change::finalize` (orchestrator) and `tests/cli.rs`
// (end-to-end with the real binary).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use specify_change::FinalizeStatus;

    #[test]
    fn passing_statuses_only_merged_and_no_branch() {
        for s in [FinalizeStatus::Merged, FinalizeStatus::NoBranch] {
            assert!(s.is_passing(), "{s} must pass");
        }
        for s in [
            FinalizeStatus::Unmerged,
            FinalizeStatus::Closed,
            FinalizeStatus::BranchPatternMismatch,
            FinalizeStatus::Dirty,
            FinalizeStatus::Failed,
        ] {
            assert!(!s.is_passing(), "{s} must refuse");
        }
    }
}
