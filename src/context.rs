use std::path::PathBuf;

use specify::{Error, PipelineView, ProjectConfig};

use crate::cli::OutputFormat;
use crate::output::CliResult;

pub struct CommandContext {
    pub format: OutputFormat,
    pub project_dir: PathBuf,
    pub config: ProjectConfig,
}

impl CommandContext {
    /// Resolve the current directory, load `.specify/project.yaml`, and
    /// bundle everything into a `CommandContext`. On failure the error is
    /// emitted (JSON or text, depending on `format`) and the appropriate
    /// exit code is returned as `Err`.
    pub fn require(format: OutputFormat) -> Result<Self, CliResult> {
        let project_dir = std::env::current_dir()
            .map_err(|e| crate::output::emit_error(format, &Error::Io(e)))?;
        let config =
            ProjectConfig::load(&project_dir).map_err(|e| crate::output::emit_error(format, &e))?;
        Ok(Self {
            format,
            project_dir,
            config,
        })
    }

    pub fn emit_error(&self, err: &Error) -> CliResult {
        crate::output::emit_error(self.format, err)
    }

    pub fn load_pipeline(&self) -> Result<PipelineView, CliResult> {
        PipelineView::load(&self.config.schema, &self.project_dir).map_err(|e| self.emit_error(&e))
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
