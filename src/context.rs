use std::path::PathBuf;

use specify::{Error, PipelineView, ProjectConfig};

use crate::cli::OutputFormat;

/// Shared context for every subcommand that operates inside an
/// initialised `.specify/` project. Created once at the top of each
/// command handler via [`CommandContext::require`].
pub struct CommandContext {
    pub format: OutputFormat,
    pub project_dir: PathBuf,
    pub config: ProjectConfig,
}

impl CommandContext {
    /// Resolve the current directory, load `.specify/project.yaml`, and
    /// bundle everything into a `CommandContext`.
    ///
    /// Returns `Err(Error)` on failure so callers can propagate with `?`.
    /// The top-level dispatcher (`run_with_project`) converts `Error` to
    /// the format-aware exit code.
    pub fn require(format: OutputFormat) -> Result<Self, Error> {
        let project_dir = std::env::current_dir().map_err(Error::Io)?;
        let config = ProjectConfig::load(&project_dir)?;
        Ok(Self {
            format,
            project_dir,
            config,
        })
    }

    /// Load the schema pipeline for this project.
    pub fn load_pipeline(&self) -> Result<PipelineView, Error> {
        PipelineView::load(&self.config.schema, &self.project_dir)
    }

    pub fn changes_dir(&self) -> PathBuf {
        ProjectConfig::changes_dir(&self.project_dir)
    }

    pub fn specs_dir(&self) -> PathBuf {
        ProjectConfig::specs_dir(&self.project_dir)
    }

    pub fn archive_dir(&self) -> PathBuf {
        ProjectConfig::archive_dir(&self.project_dir)
    }

    #[allow(dead_code)]
    pub fn specify_dir(&self) -> PathBuf {
        ProjectConfig::specify_dir(&self.project_dir)
    }
}
