pub mod change;
pub mod init;
pub mod initiative;
pub mod merge;
pub mod plan;
pub mod schema;
pub mod spec;
pub mod status;
pub mod task;
pub mod validate;
pub mod vectis;
pub mod workspace;

use specify::Error;

use crate::cli::{
    Cli, Commands, OutputFormat, SchemaAction, SpecAction, TaskAction, WorkspaceAction,
};
use crate::context::CommandContext;
use crate::output::{CliResult, emit_error};

pub fn run(cli: Cli) -> CliResult {
    match cli.command {
        Commands::Init {
            schema,
            schema_dir,
            name,
            domain,
        } => run_bare(cli.format, || init::run_init(cli.format, schema, schema_dir, name, domain)),
        Commands::Validate { change_dir } => {
            run_with_project(cli.format, |ctx| validate::run_validate(ctx, change_dir))
        }
        Commands::Merge { change_dir } => {
            run_with_project(cli.format, |ctx| merge::run_merge(ctx, change_dir))
        }
        Commands::Status { change } => {
            run_with_project(cli.format, |ctx| status::run_status(ctx, change))
        }
        Commands::Task { action } => match action {
            TaskAction::Progress { change_dir } => {
                run_with_project(cli.format, |ctx| task::run_task_progress(ctx, change_dir))
            }
            TaskAction::Mark {
                change_dir,
                task_number,
            } => run_with_project(cli.format, |ctx| {
                task::run_task_mark(ctx, change_dir, task_number)
            }),
        },
        Commands::Schema { action } => match action {
            SchemaAction::Resolve {
                schema_value,
                project_dir,
            } => run_bare(cli.format, || {
                schema::run_schema_resolve(cli.format, schema_value, project_dir)
            }),
            SchemaAction::Check { schema_dir } => {
                run_bare(cli.format, || schema::run_schema_check(cli.format, schema_dir))
            }
            SchemaAction::Pipeline { phase, change } => {
                run_with_project(cli.format, |ctx| schema::run_schema_pipeline(ctx, phase, change))
            }
        },
        Commands::Change { action } => {
            run_with_project(cli.format, |ctx| change::run_change(ctx, action))
        }
        Commands::Spec { action } => match action {
            SpecAction::Preview { change_dir } => {
                run_with_project(cli.format, |ctx| spec::run_spec_preview(ctx, change_dir))
            }
            SpecAction::ConflictCheck { change_dir } => {
                run_with_project(cli.format, |ctx| spec::run_spec_conflict_check(ctx, change_dir))
            }
        },
        Commands::Plan { action } => {
            run_with_project(cli.format, |ctx| plan::run_plan(ctx, action))
        }
        Commands::Initiative { action } => {
            run_with_project(cli.format, |ctx| initiative::run_initiative(ctx, action))
        }
        Commands::Workspace { action } => match action {
            WorkspaceAction::Sync => run_with_project(cli.format, workspace::run_workspace_sync),
            WorkspaceAction::Status => {
                run_with_project(cli.format, workspace::run_workspace_status)
            }
            WorkspaceAction::Push { projects, dry_run } => run_with_project(cli.format, |ctx| {
                workspace::run_workspace_push(ctx, projects, dry_run)
            }),
        },
        Commands::Completions { shell } => {
            let mut cmd = <crate::cli::Cli as clap::CommandFactory>::command();
            clap_complete::generate(shell, &mut cmd, "specify", &mut std::io::stdout());
            CliResult::Success
        }
        Commands::Vectis { action } => vectis::run_vectis(cli.format, &action),
    }
}

/// Run a command that requires an initialised `.specify/` project.
///
/// Loads `CommandContext` (project config + pipeline), calls `f`, and
/// maps any `Error` to the appropriate format-aware exit code. This is
/// the single error-handling boundary for project-aware commands —
/// handlers can use `?` freely inside `f`.
fn run_with_project<F>(format: OutputFormat, f: F) -> CliResult
where
    F: FnOnce(&CommandContext) -> Result<CliResult, Error>,
{
    let ctx = match CommandContext::require(format) {
        Ok(ctx) => ctx,
        Err(err) => return emit_error(format, &err),
    };
    match f(&ctx) {
        Ok(result) => result,
        Err(err) => emit_error(format, &err),
    }
}

/// Run a command that does NOT need project context but may still fail
/// with an `Error` (e.g. `schema resolve`, `schema check`).
fn run_bare<F>(format: OutputFormat, f: F) -> CliResult
where
    F: FnOnce() -> Result<CliResult, Error>,
{
    match f() {
        Ok(result) => result,
        Err(err) => emit_error(format, &err),
    }
}
