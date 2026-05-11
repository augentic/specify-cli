pub(crate) mod capability;
pub(crate) mod change;
pub(crate) mod codex;
pub(crate) mod compatibility;
pub(crate) mod context;
mod init;
pub(crate) mod registry;
pub(crate) mod slice;
mod status;
pub(crate) mod tool;
pub(crate) mod workspace;

use clap::CommandFactory;
use specify_error::Result;

use crate::cli::{CapabilityAction, Cli, Commands, Format, ToolAction, WorkspaceAction};
use crate::context::Ctx;
use crate::output::{Exit, report};

pub(crate) fn run(cli: Cli) -> Exit {
    let format = cli.format;
    match cli.command {
        Commands::Init {
            capability,
            name,
            domain,
            hub,
        } => {
            dispatch(format, || init::run(format, capability.as_deref(), name.as_deref(), domain.as_deref(), hub))
        }
        Commands::Status => scoped(format, status::run),
        Commands::Context { action } => scoped(format, |ctx| context::run(ctx, &action)),
        Commands::Capability { action } => match action {
            CapabilityAction::Resolve {
                capability_value,
                project_dir,
            } => dispatch(format, || capability::resolve(format, capability_value, &project_dir)),
            CapabilityAction::Check { capability_dir } => {
                dispatch(format, || capability::check(format, &capability_dir))
            }
            CapabilityAction::Pipeline { phase, slice } => {
                scoped(format, |ctx| capability::pipeline(ctx, phase, slice.as_deref()))
            }
        },
        Commands::Codex { action } => scoped(format, |ctx| codex::run(ctx, action)),
        Commands::Compatibility { action } => scoped(format, |ctx| compatibility::run(ctx, action)),
        Commands::Tool { action } => match action {
            ToolAction::Run { name, args } => run_tool(format, &name, args),
            ToolAction::List => scoped(format, tool::list),
            ToolAction::Fetch { name } => scoped(format, |ctx| tool::fetch(ctx, name.as_deref())),
            ToolAction::Show { name } => scoped(format, |ctx| tool::show(ctx, &name)),
            ToolAction::Gc => scoped(format, tool::gc),
        },
        Commands::Slice { action } => scoped(format, |ctx| slice::run(ctx, action)),
        Commands::Change { action } => scoped(format, |ctx| change::run(ctx, action)),
        Commands::Registry { action } => scoped(format, |ctx| registry::run(ctx, action)),
        Commands::Completions { shell } => {
            let mut cmd = Cli::command();
            clap_complete::generate(shell, &mut cmd, "specify", &mut std::io::stdout());
            Exit::Success
        }
        Commands::Workspace { action } => match action {
            WorkspaceAction::Sync { projects } => {
                scoped(format, |ctx| workspace::sync(ctx, &projects))
            }
            WorkspaceAction::Status { projects } => {
                scoped(format, |ctx| workspace::status(ctx, &projects))
            }
            WorkspaceAction::PrepareBranch {
                project,
                change,
                sources,
                outputs,
            } => scoped(format, |ctx| {
                workspace::prepare_branch(ctx, &project, change, sources, outputs)
            }),
            WorkspaceAction::Push { projects, dry_run } => {
                scoped(format, |ctx| workspace::push(ctx, &projects, dry_run))
            }
        },
    }
}

/// Run a command that requires an initialised `.specify/` project.
///
/// Loads `Ctx` (project config + pipeline), calls `f`, and maps any
/// `Error` to the appropriate format-aware exit code via
/// [`report`]. This is the single error-handling boundary for
/// project-aware handlers — they can use `?` freely inside `f`.
fn scoped<F>(format: Format, f: F) -> Exit
where
    F: FnOnce(&Ctx) -> Result<()>,
{
    let ctx = match Ctx::load(format) {
        Ok(ctx) => ctx,
        Err(err) => return report(format, &err),
    };
    match f(&ctx) {
        Ok(()) => Exit::Success,
        Err(err) => report(format, &err),
    }
}

/// Run a command that does NOT need project context but may still fail
/// with an `Error` (e.g. `capability resolve`, `capability check`).
/// The `Ctx`-bearing peer is [`scoped`].
fn dispatch<F>(format: Format, f: F) -> Exit
where
    F: FnOnce() -> Result<()>,
{
    match f() {
        Ok(()) => Exit::Success,
        Err(err) => report(format, &err),
    }
}

/// `tool run` is the only handler that mints a [`Exit::Code`]
/// exit — the WASI guest's exit byte is forwarded verbatim so
/// `specify tool run` is a transparent shim. Handled outside the
/// `Result<()>` channel because the success branch carries the
/// guest's exit code rather than collapsing to `Success`.
fn run_tool(format: Format, name: &str, args: Vec<String>) -> Exit {
    let ctx = match Ctx::load(format) {
        Ok(ctx) => ctx,
        Err(err) => return report(format, &err),
    };
    match tool::run(&ctx, name, args) {
        Ok(0) => Exit::Success,
        Ok(code) => Exit::Code(code),
        Err(err) => report(format, &err),
    }
}
