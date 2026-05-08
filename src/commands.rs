mod capability;
mod change;
mod init;
mod migrate;
mod registry;
mod slice;
mod status;
mod tool;
mod workspace;

use specify::Error;

use crate::cli::{
    CapabilityAction, Cli, Commands, MigrateAction, OutputFormat, ToolAction, WorkspaceAction,
};
use crate::context::CommandContext;
use crate::output::{CliResult, emit_error};

pub fn run(cli: Cli) -> CliResult {
    match cli.command {
        Commands::Init {
            capability,
            name,
            domain,
            hub,
        } => run_bare(cli.format, || init::run_init(cli.format, capability, name, domain, hub)),
        Commands::Status => run_with_project(cli.format, status::run_status_dashboard),
        Commands::Capability { action } => match action {
            CapabilityAction::Resolve {
                capability_value,
                project_dir,
            } => run_bare(cli.format, || {
                capability::run_capability_resolve(cli.format, capability_value, project_dir)
            }),
            CapabilityAction::Check { capability_dir } => run_bare(cli.format, || {
                capability::run_capability_check(cli.format, capability_dir)
            }),
            CapabilityAction::Pipeline { phase, slice } => run_with_project(cli.format, |ctx| {
                capability::run_capability_pipeline(ctx, phase, slice)
            }),
        },
        Commands::Tool { action } => match action {
            ToolAction::Run { name, args } => {
                run_with_project(cli.format, |ctx| tool::run_tool_run(ctx, name, args))
            }
            ToolAction::List => run_with_project(cli.format, tool::run_tool_list),
            ToolAction::Fetch { name } => {
                run_with_project(cli.format, |ctx| tool::run_tool_fetch(ctx, name))
            }
            ToolAction::Show { name } => {
                run_with_project(cli.format, |ctx| tool::run_tool_show(ctx, name))
            }
            ToolAction::Gc { all } => {
                run_with_project(cli.format, |ctx| tool::run_tool_gc(ctx, all))
            }
        },
        Commands::Slice { action } => {
            run_with_project(cli.format, |ctx| slice::run_slice(ctx, action))
        }
        Commands::Change { action } => {
            run_with_project(cli.format, |ctx| change::run_change(ctx, action))
        }
        Commands::Registry { action } => {
            run_with_project(cli.format, |ctx| registry::run_registry(ctx, action))
        }
        Commands::Workspace { action } => match action {
            WorkspaceAction::Sync { projects } => {
                run_with_project(cli.format, |ctx| workspace::run_workspace_sync(ctx, projects))
            }
            WorkspaceAction::Status { projects } => {
                run_with_project(cli.format, |ctx| workspace::run_workspace_status(ctx, projects))
            }
            WorkspaceAction::PrepareBranch {
                project,
                change,
                sources,
                outputs,
            } => run_with_project(cli.format, |ctx| {
                workspace::run_workspace_prepare_branch(ctx, project, change, sources, outputs)
            }),
            WorkspaceAction::Push { projects, dry_run } => run_with_project(cli.format, |ctx| {
                workspace::run_workspace_push(ctx, projects, dry_run)
            }),
            WorkspaceAction::Merge { projects, dry_run } => run_bare(cli.format, || {
                workspace::run_workspace_merge_removed(cli.format, projects, dry_run)
            }),
        },
        Commands::Migrate { action } => match action {
            MigrateAction::V2Layout { dry_run } => run_bare(cli.format, || {
                let cwd = std::env::current_dir().map_err(Error::Io)?;
                migrate::run_migrate_v2_layout(cli.format, &cwd, dry_run)
            }),
            MigrateAction::SliceLayout { dry_run } => run_bare(cli.format, || {
                let cwd = std::env::current_dir().map_err(Error::Io)?;
                migrate::run_migrate_slice_layout(cli.format, &cwd, dry_run)
            }),
            MigrateAction::ChangeNoun { dry_run } => run_bare(cli.format, || {
                let cwd = std::env::current_dir().map_err(Error::Io)?;
                migrate::run_migrate_change_noun(cli.format, &cwd, dry_run)
            }),
        },
        Commands::Completions { shell } => {
            let mut cmd = <crate::cli::Cli as clap::CommandFactory>::command();
            clap_complete::generate(shell, &mut cmd, "specify", &mut std::io::stdout());
            CliResult::Success
        }
    }
}

/// Run a command that requires an initialised `.specify/` project.
///
/// Loads `CommandContext` (project config + pipeline), runs the
/// hard-cutover detector for v1 layout artifacts, calls `f`, and
/// maps any `Error` to the appropriate format-aware exit code. This
/// is the single error-handling boundary for project-aware commands —
/// handlers can use `?` freely inside `f`. The v1-layout check is
/// the choke point that surfaces `Error::LegacyLayout` (and points
/// the operator at `specify migrate v2-layout`) for every verb that
/// touches project state.
fn run_with_project<F>(format: OutputFormat, f: F) -> CliResult
where
    F: FnOnce(&CommandContext) -> Result<CliResult, Error>,
{
    let ctx = match CommandContext::require(format) {
        Ok(ctx) => ctx,
        Err(err) => return emit_error(format, &err),
    };
    let legacy = specify::detect_legacy_layout(&ctx.project_dir);
    if !legacy.is_empty() {
        return emit_error(format, &Error::LegacyLayout { paths: legacy });
    }
    match f(&ctx) {
        Ok(result) => result,
        Err(err) => emit_error(format, &err),
    }
}

/// Run a command that does NOT need project context but may still fail
/// with an `Error` (e.g. `capability resolve`, `capability check`).
fn run_bare<F>(format: OutputFormat, f: F) -> CliResult
where
    F: FnOnce() -> Result<CliResult, Error>,
{
    match f() {
        Ok(result) => result,
        Err(err) => emit_error(format, &err),
    }
}
