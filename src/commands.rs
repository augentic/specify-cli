pub mod context;
mod init;
pub mod plan;
pub mod registry;
pub mod slice;
pub mod source;
pub mod target;
pub mod tool;
pub mod workspace;

use std::io::Write;
use std::path::Path;

use clap::CommandFactory;
use serde::Serialize;
use specify_domain::adapter::{Axis, SourceAdapter, TargetAdapter};
use specify_error::Result;

use crate::cli::{Cli, Commands, Format};
use crate::commands::source::cli::SourceAction;
use crate::commands::target::cli::TargetAction;
use crate::commands::tool::cli::ToolAction;
use crate::commands::workspace::cli::WorkspaceAction;
use crate::context::Ctx;
use crate::output::{self, Exit, report};

pub fn run(cli: Cli) -> Exit {
    let format = cli.format;
    match cli.command {
        Commands::Init {
            adapter,
            name,
            domain,
            hub,
        } => dispatch(format, || {
            init::run(format, adapter.as_deref(), name.as_deref(), domain.as_deref(), hub)
        }),
        Commands::Source { action } => match action {
            SourceAction::Resolve {
                name,
                project_dir,
                explain,
            } => dispatch(format, || {
                if explain {
                    source::cache::explain(format, &name, &project_dir)
                } else {
                    resolve_adapter(format, Axis::Source, &name, &project_dir)
                }
            }),
        },
        Commands::Target { action } => match action {
            TargetAction::Resolve { value, project_dir } => {
                dispatch(format, || resolve_adapter(format, Axis::Target, &value, &project_dir))
            }
        },
        Commands::Tool { action } => match action {
            ToolAction::Run { name, args } => run_tool(format, &name, args),
            ToolAction::Fetch { name } => scoped(format, |ctx| tool::fetch(ctx, name.as_deref())),
            ToolAction::Gc => scoped(format, tool::gc),
        },
        Commands::Slice { action } => scoped(format, |ctx| slice::run(ctx, action)),
        Commands::Plan { action } => scoped(format, |ctx| plan::run(ctx, action)),
        Commands::Registry { action } => scoped(format, |ctx| registry::run(ctx, action)),
        Commands::Completions { shell } => {
            let mut cmd = Cli::command();
            clap_complete::generate(shell, &mut cmd, "specrun", &mut std::io::stdout());
            Exit::Success
        }
        Commands::Workspace { action } => match action {
            WorkspaceAction::Sync { projects } => {
                scoped(format, |ctx| workspace::sync(ctx, &projects))
            }
            WorkspaceAction::Prepare {
                project,
                change,
                sources,
                outputs,
            } => scoped(format, |ctx| workspace::prepare(ctx, &project, change, sources, outputs)),
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

/// Run a command that does NOT need project context but may still
/// fail with an `Error` (e.g. `source resolve` / `target resolve`).
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

/// `tool run` is the only handler that mints a [`Exit::Code`] exit;
/// see [DECISIONS.md §"Exit codes"](../DECISIONS.md#exit-codes) for
/// the rationale. Handled outside the `Result<()>` channel so the
/// success branch can carry the guest's exit code rather than
/// collapsing to `Success`.
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

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct ResolveBody {
    axis: &'static str,
    name: String,
    resolved_path: String,
    location: &'static str,
    operations: Vec<String>,
    description: Option<String>,
}

fn write_resolve_text(w: &mut dyn Write, body: &ResolveBody) -> std::io::Result<()> {
    writeln!(w, "{}", body.resolved_path)?;
    writeln!(w, "  axis: {}", body.axis)?;
    writeln!(w, "  name: {}", body.name)?;
    writeln!(w, "  location: {}", body.location)?;
    writeln!(w, "  operations: {}", body.operations.join(", "))?;
    if let Some(desc) = &body.description {
        writeln!(w, "  description: {desc}")?;
    }
    Ok(())
}

/// Resolve a source- or target-adapter manifest by kebab name and emit
/// the wire-stable [`ResolveBody`] envelope. Probe order matches the
/// axis-specific resolver: agent-populated manifest cache at
/// `<project_dir>/.specify/.cache/manifests/{sources,targets}/<name>/`
/// first, then the in-repo `<project_dir>/adapters/{sources,targets}/<name>/`.
///
/// For [`Axis::Target`], `value` accepts either `<name>` or
/// `<name>@<version>`; the `@version` suffix is treated as an opaque
/// identifier and stripped to leave the kebab name for the lookup
/// (workflow §CLI surface).
fn resolve_adapter(format: Format, axis: Axis, value: &str, project_dir: &Path) -> Result<()> {
    let body = match axis {
        Axis::Source => {
            let resolved = SourceAdapter::resolve(value, project_dir)?;
            ResolveBody {
                axis: axis.dir_segment(),
                name: resolved.manifest.name.clone(),
                resolved_path: resolved.location.path().display().to_string(),
                location: resolved.location.label(),
                operations: resolved.manifest.operations().map(ToString::to_string).collect(),
                description: resolved.manifest.description.clone(),
            }
        }
        Axis::Target => {
            let name = value.split_once('@').map_or(value, |(n, _)| n);
            let resolved = TargetAdapter::resolve(name, project_dir)?;
            ResolveBody {
                axis: axis.dir_segment(),
                name: resolved.manifest.name.clone(),
                resolved_path: resolved.location.path().display().to_string(),
                location: resolved.location.label(),
                operations: resolved.manifest.operations().map(ToString::to_string).collect(),
                description: resolved.manifest.description.clone(),
            }
        }
    };
    output::emit(Box::new(std::io::stdout().lock()), format, &body, write_resolve_text)?;
    Ok(())
}
