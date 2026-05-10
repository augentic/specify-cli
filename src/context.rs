use std::path::PathBuf;

use specify_capability::PipelineView;
use specify_config::ProjectConfig;
use specify_error::Error;

use crate::cli::OutputFormat;

/// Shared context for every subcommand that operates inside an
/// initialised `.specify/` project. Created once at the top of each
/// command handler via [`CommandContext::load`].
pub struct CommandContext {
    pub format: OutputFormat,
    pub project_dir: PathBuf,
    pub config: ProjectConfig,
}

impl CommandContext {
    /// Resolve the current project root, load `.specify/project.yaml`,
    /// and bundle everything into a `CommandContext`.
    ///
    /// Returns `Err(Error)` on failure so callers can propagate with `?`.
    /// The top-level dispatcher (`run_with_project`) converts `Error` to
    /// the format-aware exit code.
    pub fn load(format: OutputFormat) -> Result<Self, Error> {
        let current_dir = std::env::current_dir().map_err(Error::Io)?;
        let project_dir = ProjectConfig::find_root(&current_dir)?.ok_or(Error::NotInitialized)?;
        let config = ProjectConfig::load(&project_dir)?;
        Ok(Self {
            format,
            project_dir,
            config,
        })
    }

    /// Load the capability pipeline for this project.
    ///
    /// Hub projects (`hub: true`, `capability:` omitted) do not declare
    /// a capability and have no pipeline to walk, so this returns a
    /// `hub-no-capability` diagnostic naming the hub case rather than a
    /// stray capability-resolution error lower down the stack.
    pub fn load_pipeline(&self) -> Result<PipelineView, Error> {
        let Some(capability) = self.config.capability.as_deref() else {
            return Err(Error::Diag {
                code: "hub-no-capability",
                detail: "this project has no capability declared (hub projects do not run \
                         phase pipelines); only `specify registry` and `specify change` \
                         verbs are supported on hubs"
                    .to_string(),
            });
        };
        PipelineView::load(capability, &self.project_dir)
    }

    pub fn slices_dir(&self) -> PathBuf {
        ProjectConfig::slices_dir(&self.project_dir)
    }

    pub fn archive_dir(&self) -> PathBuf {
        ProjectConfig::archive_dir(&self.project_dir)
    }
}
