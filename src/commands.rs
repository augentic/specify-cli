// Clap dispatch hands owned subcommand values down through every
// handler module — promote the lint waiver here rather than repeat it
// per file.
#![allow(
    clippy::needless_pass_by_value,
    reason = "Clap dispatch hands owned subcommand values to these command handlers."
)]

pub mod capability;
pub mod change;
pub mod codex;
pub mod compatibility;
pub mod context;
mod init;
pub mod registry;
pub mod slice;
mod status;
pub mod tool;
pub mod workspace;

use specify_error::Result;

use crate::cli::{CapabilityAction, Cli, Commands, OutputFormat, ToolAction, WorkspaceAction};
use crate::context::Ctx;
use crate::output::{CliResult, report_error};

/// Map a handler's success payload onto a [`CliResult`] exit code.
///
/// Lets the dispatcher accept both `Result<()>` (the common case —
/// success is unconditional and maps to `CliResult::Success`) and
/// `Result<CliResult>` (handlers that conditionally surface a
/// non-success exit like `GenericFailure` / `ValidationFailed`).
trait IntoCliResult {
    fn into_cli_result(self) -> CliResult;
}

impl IntoCliResult for CliResult {
    fn into_cli_result(self) -> CliResult {
        self
    }
}

impl IntoCliResult for () {
    fn into_cli_result(self) -> CliResult {
        CliResult::Success
    }
}

pub fn run(cli: Cli) -> CliResult {
    let format = cli.format;
    match cli.command {
        Commands::Init {
            capability,
            name,
            domain,
            hub,
        } => unscoped(format, || init::run(format, capability, name, domain, hub)),
        Commands::Status => with_project(format, status::run),
        Commands::Context { action } => with_project(format, |ctx| context::run(ctx, action)),
        Commands::Capability { action } => match action {
            CapabilityAction::Resolve {
                capability_value,
                project_dir,
            } => unscoped(format, || capability::resolve(format, capability_value, project_dir)),
            CapabilityAction::Check { capability_dir } => {
                unscoped(format, || capability::check(format, capability_dir))
            }
            CapabilityAction::Pipeline { phase, slice } => {
                with_project(format, |ctx| capability::pipeline(ctx, phase, slice))
            }
        },
        Commands::Codex { action } => with_project(format, |ctx| codex::run(ctx, action)),
        Commands::Compatibility { action } => {
            with_project(format, |ctx| compatibility::run(ctx, action))
        }
        Commands::Tool { action } => match action {
            ToolAction::Run { name, args } => {
                with_project(format, |ctx| tool::run(ctx, name, args))
            }
            ToolAction::List => with_project(format, tool::list),
            ToolAction::Fetch { name } => with_project(format, |ctx| tool::fetch(ctx, name)),
            ToolAction::Show { name } => with_project(format, |ctx| tool::show(ctx, name)),
            ToolAction::Gc => with_project(format, tool::gc),
        },
        Commands::Slice { action } => with_project(format, |ctx| slice::run(ctx, action)),
        Commands::Change { action } => with_project(format, |ctx| change::run(ctx, action)),
        Commands::Registry { action } => with_project(format, |ctx| registry::run(ctx, action)),
        Commands::Workspace { action } => match action {
            WorkspaceAction::Sync { projects } => {
                with_project(format, |ctx| workspace::sync(ctx, projects))
            }
            WorkspaceAction::Status { projects } => {
                with_project(format, |ctx| workspace::status(ctx, projects))
            }
            WorkspaceAction::PrepareBranch {
                project,
                change,
                sources,
                outputs,
            } => with_project(format, |ctx| {
                workspace::prepare_branch(ctx, project, change, sources, outputs)
            }),
            WorkspaceAction::Push { projects, dry_run } => {
                with_project(format, |ctx| workspace::push(ctx, projects, dry_run))
            }
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
/// Loads `Ctx` (project config + pipeline), calls `f`, and
/// maps any `Error` to the appropriate format-aware exit code. This
/// is the single error-handling boundary for project-aware commands —
/// handlers can use `?` freely inside `f`. Handlers that return
/// `Result<()>` collapse to `CliResult::Success` here; handlers that
/// return `Result<CliResult>` flow through unchanged (the
/// non-success-exit case).
fn with_project<F, R>(format: OutputFormat, f: F) -> CliResult
where
    F: FnOnce(&Ctx) -> Result<R>,
    R: IntoCliResult,
{
    let ctx = match Ctx::load(format) {
        Ok(ctx) => ctx,
        Err(err) => return report_error(format, &err),
    };
    match f(&ctx) {
        Ok(value) => value.into_cli_result(),
        Err(err) => report_error(format, &err),
    }
}

/// Run a command that does NOT need project context but may still fail
/// with an `Error` (e.g. `capability resolve`, `capability check`).
fn unscoped<F, R>(format: OutputFormat, f: F) -> CliResult
where
    F: FnOnce() -> Result<R>,
    R: IntoCliResult,
{
    match f() {
        Ok(value) => value.into_cli_result(),
        Err(err) => report_error(format, &err),
    }
}
