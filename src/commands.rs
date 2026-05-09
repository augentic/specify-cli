mod capability;
mod change;
mod codex;
mod context;
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
        } => bare(cli.format, || init::run(cli.format, capability, name, domain, hub)),
        Commands::Status => with_project(cli.format, status::run),
        Commands::Context { action } => with_project(cli.format, |ctx| context::run(ctx, action)),
        Commands::Capability { action } => match action {
            CapabilityAction::Resolve {
                capability_value,
                project_dir,
            } => {
                bare(cli.format, || capability::resolve(cli.format, capability_value, project_dir))
            }
            CapabilityAction::Check { capability_dir } => {
                bare(cli.format, || capability::check(cli.format, capability_dir))
            }
            CapabilityAction::Pipeline { phase, slice } => {
                with_project(cli.format, |ctx| capability::pipeline(ctx, phase, slice))
            }
        },
        Commands::Codex { action } => with_project(cli.format, |ctx| codex::run(ctx, action)),
        Commands::Tool { action } => match action {
            ToolAction::Run { name, args } => {
                with_project(cli.format, |ctx| tool::run(ctx, name, args))
            }
            ToolAction::List => with_project(cli.format, tool::list),
            ToolAction::Fetch { name } => with_project(cli.format, |ctx| tool::fetch(ctx, name)),
            ToolAction::Show { name } => with_project(cli.format, |ctx| tool::show(ctx, name)),
            ToolAction::Gc { all } => with_project(cli.format, |ctx| tool::gc(ctx, all)),
        },
        Commands::Slice { action } => with_project(cli.format, |ctx| slice::run(ctx, action)),
        Commands::Change { action } => with_project(cli.format, |ctx| change::run(ctx, action)),
        Commands::Registry { action } => with_project(cli.format, |ctx| registry::run(ctx, action)),
        Commands::Workspace { action } => match action {
            WorkspaceAction::Sync { projects } => {
                with_project(cli.format, |ctx| workspace::sync(ctx, projects))
            }
            WorkspaceAction::Status { projects } => {
                with_project(cli.format, |ctx| workspace::status(ctx, projects))
            }
            WorkspaceAction::PrepareBranch {
                project,
                change,
                sources,
                outputs,
            } => with_project(cli.format, |ctx| {
                workspace::prepare_branch(ctx, project, change, sources, outputs)
            }),
            WorkspaceAction::Push { projects, dry_run } => {
                with_project(cli.format, |ctx| workspace::push(ctx, projects, dry_run))
            }
            WorkspaceAction::Merge { projects, dry_run } => {
                bare(cli.format, || workspace::merge_removed(cli.format, projects, dry_run))
            }
        },
        Commands::Migrate { action } => match action {
            MigrateAction::V2Layout { dry_run } => bare(cli.format, || {
                let cwd = std::env::current_dir().map_err(Error::Io)?;
                migrate::v2_layout(cli.format, &cwd, dry_run)
            }),
            MigrateAction::SliceLayout { dry_run } => bare(cli.format, || {
                let cwd = std::env::current_dir().map_err(Error::Io)?;
                migrate::slice_layout(cli.format, &cwd, dry_run)
            }),
            MigrateAction::ChangeNoun { dry_run } => bare(cli.format, || {
                let cwd = std::env::current_dir().map_err(Error::Io)?;
                migrate::change_noun(cli.format, &cwd, dry_run)
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
fn with_project<F>(format: OutputFormat, f: F) -> CliResult
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
fn bare<F>(format: OutputFormat, f: F) -> CliResult
where
    F: FnOnce() -> Result<CliResult, Error>,
{
    match f() {
        Ok(result) => result,
        Err(err) => emit_error(format, &err),
    }
}
