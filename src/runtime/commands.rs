pub mod agents;
pub mod archive;
mod init;
pub mod journal;
pub mod lint;
mod migrate;
pub mod plan;
pub mod plugins;
pub mod registry;
pub mod rules;
pub mod slice;
pub mod source;
pub mod target;
pub mod tool;
mod upgrade;
pub mod workspace;

use std::io::Write;
use std::path::{Path, PathBuf};

use clap::CommandFactory;
use serde::Serialize;
use specify_error::Result;
use specify_workflow::adapter::{Axis, SourceAdapter, TargetAdapter};

use crate::runtime::cli::{Cli, Commands, Format};
use crate::runtime::commands::journal::cli::JournalAction;
use crate::runtime::commands::lint::cli::LintAction;
use crate::runtime::commands::rules::cli::RulesAction;
use crate::runtime::commands::source::cli::SourceAction;
use crate::runtime::commands::target::cli::TargetAction;
use crate::runtime::commands::tool::cli::ToolAction;
use crate::runtime::commands::workspace::cli::WorkspaceAction;
use crate::runtime::context::Ctx;
use crate::runtime::output::{self, Exit, report};

pub fn run(cli: Cli) -> Exit {
    let format = cli.format;
    match cli.command {
        Commands::Init {
            adapter,
            name,
            description,
            workspace,
            include_framework,
            platforms,
            check_migration,
            upgrade,
        } => dispatch(format, || {
            init::run(&init::Args {
                format,
                adapter: adapter.as_deref(),
                name: name.as_deref(),
                description: description.as_deref(),
                workspace,
                include_framework,
                platforms: platforms.as_deref(),
                check_migration,
                upgrade,
            })
        }),
        Commands::Source { action } => dispatch_source(format, action),
        Commands::Target { action } => match action {
            TargetAction::Resolve { value, project_dir } => {
                dispatch(format, || resolve_adapter(format, Axis::Target, &value, &project_dir))
            }
        },
        Commands::Rules { action } => match action {
            RulesAction::Export(args) => dispatch(format, || rules::export::run(format, &args)),
            RulesAction::Sync(args) => scoped(format, |ctx| rules::sync::run(ctx, &args)),
        },
        Commands::Tool { action } => match action {
            ToolAction::Run { name, args } => run_tool_with(format, &name, args),
            ToolAction::Fetch { name } => scoped(format, |ctx| tool::fetch(ctx, name.as_deref())),
            ToolAction::Gc => scoped(format, tool::gc),
            ToolAction::Schema { name, schema } => {
                run_tool_with(format, &name, vec!["schema".to_string(), schema])
            }
        },
        Commands::Lint { action } => match action {
            LintAction::Product(args) => {
                scoped_at(format, &args.project_dir, |ctx| lint::product::run(ctx, &args))
            }
            LintAction::Framework(args) => dispatch(format, || lint::framework::run(format, &args)),
        },
        Commands::Journal { action } => match action {
            JournalAction::Emit { event, payload } => {
                scoped(format, |ctx| journal::emit::emit(ctx, &event, payload.as_deref()))
            }
        },
        Commands::Slice { action } => scoped(format, |ctx| slice::run(ctx, action)),
        Commands::Archive { action } => scoped(format, |ctx| archive::run(ctx, &action)),
        Commands::Plan { action } => scoped(format, |ctx| plan::run(ctx, action)),
        Commands::Registry { action } => scoped(format, |ctx| registry::run(ctx, action)),
        Commands::Completions { shell } => {
            let mut cmd = Cli::command();
            clap_complete::generate(shell, &mut cmd, "specify", &mut std::io::stdout());
            Exit::Success
        }
        Commands::Migrate {
            from,
            to,
            dry_run,
            yes,
        } => {
            dispatch(format, || migrate::run(format, from.as_deref(), to.as_deref(), dry_run, yes))
        }
        Commands::Upgrade {
            channel,
            yes,
            dry_run,
        } => dispatch(format, || upgrade::run(format, channel, yes, dry_run)),
        Commands::Plugins { action } => dispatch(format, || plugins::run(format, action)),
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

/// Dispatch the `specify source {resolve, preview, survey, extract}`
/// family.
///
/// Factored out of [`run`] so the top-level dispatcher stays under the
/// per-function line budget; the arms keep their distinct context
/// posture — `resolve` / `preview` are project-context-free
/// ([`dispatch`]), `survey` / `extract` are project-scoped
/// ([`scoped`]).
fn dispatch_source(format: Format, action: SourceAction) -> Exit {
    match action {
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
        SourceAction::Preview {
            adapter,
            source,
            lead,
            out,
            project_dir,
        } => dispatch(format, || {
            source::preview::preview(format, &adapter, &source, &lead, out.as_deref(), &project_dir)
        }),
        SourceAction::Survey { source, plan, phase } => {
            scoped(format, |ctx| source::survey::run(ctx, &source, plan.as_deref(), phase))
        }
        SourceAction::Extract {
            source,
            lead,
            slice,
            phase,
        } => scoped(format, |ctx| source::extract::run(ctx, &source, &lead, &slice, phase)),
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

/// Variant of [`scoped`] that loads `Ctx` against an explicit
/// project directory instead of the process CWD. Used by handlers
/// that take a `--project-dir` flag (e.g. `specify lint`).
fn scoped_at<F>(format: Format, project_dir: &Path, f: F) -> Exit
where
    F: FnOnce(&Ctx) -> Result<()>,
{
    let ctx = match Ctx::load_at(format, project_dir) {
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

/// Tool execution is the only handler path that mints a [`Exit::Code`] exit;
/// see [DECISIONS.md §"Exit codes"](../DECISIONS.md#exit-codes) for
/// the rationale. Handled outside the `Result<()>` channel so the
/// success branch can carry the guest's exit code rather than
/// collapsing to `Success`.
fn run_tool_with(format: Format, name: &str, args: Vec<String>) -> Exit {
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

/// Directory segment under a resolved adapter root that holds the
/// brief markdown files. Manifest brief paths are relative and join
/// onto `<adapter-root>/briefs/`. Shared with the source prep seam
/// ([`source::prep`]) so the C1 `briefs-dir` is computed in one place.
pub const BRIEFS_DIR: &str = "briefs";

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct ResolveBody {
    axis: &'static str,
    name: String,
    resolved_path: String,
    /// Absolute path to the resolved adapter's `briefs/` directory —
    /// `<resolved-path>/briefs`. Brief paths in the manifest are
    /// relative (e.g. `briefs/extract.md`) and join onto this root.
    briefs_dir: PathBuf,
    location: &'static str,
    operations: Vec<String>,
    description: Option<String>,
}

fn write_resolve_text(w: &mut dyn Write, body: &ResolveBody) -> std::io::Result<()> {
    writeln!(w, "{}", body.resolved_path)?;
    writeln!(w, "  axis: {}", body.axis)?;
    writeln!(w, "  name: {}", body.name)?;
    writeln!(w, "  briefs-dir: {}", body.briefs_dir.display())?;
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
    // Common envelope shape; only the per-axis resolver and the
    // `@version` strip (target-only) differ. `briefs_dir` is the
    // resolved adapter root joined with `briefs/` — the directory the
    // manifest's relative brief paths join onto (preview.rs:68).
    let (name, resolved_path, briefs_dir, location, operations, description) = match axis {
        Axis::Source => {
            let resolved = SourceAdapter::resolve(value, project_dir)?;
            let operations = resolved.manifest.operations().map(ToString::to_string).collect();
            let briefs_dir = resolved.location.path().join(BRIEFS_DIR);
            let resolved_path = resolved.location.path().display().to_string();
            let location = resolved.location.label();
            (
                resolved.manifest.name,
                resolved_path,
                briefs_dir,
                location,
                operations,
                resolved.manifest.description,
            )
        }
        Axis::Target => {
            let name = value.split_once('@').map_or(value, |(n, _)| n);
            let resolved = TargetAdapter::resolve(name, project_dir)?;
            let operations = resolved.manifest.operations().map(ToString::to_string).collect();
            let briefs_dir = resolved.location.path().join(BRIEFS_DIR);
            let resolved_path = resolved.location.path().display().to_string();
            let location = resolved.location.label();
            (
                resolved.manifest.name,
                resolved_path,
                briefs_dir,
                location,
                operations,
                resolved.manifest.description,
            )
        }
    };
    let body = ResolveBody {
        axis: axis.dir_segment(),
        name,
        resolved_path,
        briefs_dir,
        location,
        operations,
        description,
    };
    output::emit(&mut std::io::stdout().lock(), format, &body, write_resolve_text)?;
    Ok(())
}
