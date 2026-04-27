pub(crate) mod change;
pub(crate) mod init;
pub(crate) mod initiative;
pub(crate) mod merge;
pub(crate) mod plan;
pub(crate) mod schema;
pub(crate) mod spec;
pub(crate) mod status;
pub(crate) mod task;
pub(crate) mod validate;
pub(crate) mod vectis;
pub(crate) mod workspace;

use std::path::PathBuf;

use specify::{Error, ProjectConfig};

use crate::cli::{
    Cli, Commands, SchemaAction, SpecAction, TaskAction, WorkspaceAction,
};
use crate::output::CliResult;

pub(crate) fn run(cli: Cli) -> CliResult {
    match cli.command {
        Commands::Init {
            schema,
            schema_dir,
            name,
            domain,
        } => init::run_init(cli.format, schema, schema_dir, name, domain),
        Commands::Validate { change_dir } => validate::run_validate(cli.format, change_dir),
        Commands::Merge { change_dir } => merge::run_merge(cli.format, change_dir),
        Commands::Status { change } => status::run_status(cli.format, change),
        Commands::Task { action } => match action {
            TaskAction::Progress { change_dir } => task::run_task_progress(cli.format, change_dir),
            TaskAction::Mark {
                change_dir,
                task_number,
            } => task::run_task_mark(cli.format, change_dir, task_number),
        },
        Commands::Schema { action } => match action {
            SchemaAction::Resolve {
                schema_value,
                project_dir,
            } => schema::run_schema_resolve(cli.format, schema_value, project_dir),
            SchemaAction::Check { schema_dir } => schema::run_schema_check(cli.format, schema_dir),
            SchemaAction::Pipeline { phase, change } => {
                schema::run_schema_pipeline(cli.format, phase, change)
            }
        },
        Commands::Change { action } => change::run_change(cli.format, action),
        Commands::Spec { action } => match action {
            SpecAction::Preview { change_dir } => spec::run_spec_preview(cli.format, change_dir),
            SpecAction::ConflictCheck { change_dir } => {
                spec::run_spec_conflict_check(cli.format, change_dir)
            }
        },
        Commands::Plan { action } => plan::run_plan(cli.format, action),
        Commands::Initiative { action } => initiative::run_initiative(cli.format, action),
        Commands::Workspace { action } => match action {
            WorkspaceAction::Sync => workspace::run_initiative_workspace_sync(cli.format),
            WorkspaceAction::Status => workspace::run_initiative_workspace_status(cli.format),
            WorkspaceAction::Push {
                projects,
                dry_run,
            } => workspace::run_workspace_push(cli.format, projects, dry_run),
        },
        Commands::Vectis { action } => vectis::run_vectis(cli.format, &action),
    }
}

fn current_dir() -> Result<PathBuf, Error> {
    std::env::current_dir().map_err(Error::Io)
}

/// Load `.specify/project.yaml` from the current directory, running
/// the CLI version-floor check in the process. Every subcommand that
/// touches `.specify/` routes through this so the error shape for
/// "not initialised" / "CLI too old" is uniform.
pub(super) fn require_project() -> Result<(PathBuf, ProjectConfig), Error> {
    let project_dir = current_dir()?;
    let config = ProjectConfig::load(&project_dir)?;
    Ok((project_dir, config))
}

