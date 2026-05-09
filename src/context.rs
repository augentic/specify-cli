use std::path::{Path, PathBuf};

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
    /// Resolve the current project root, load `.specify/project.yaml`,
    /// and bundle everything into a `CommandContext`.
    ///
    /// Returns `Err(Error)` on failure so callers can propagate with `?`.
    /// The top-level dispatcher (`run_with_project`) converts `Error` to
    /// the format-aware exit code.
    pub fn require(format: OutputFormat) -> Result<Self, Error> {
        let current_dir = std::env::current_dir().map_err(Error::Io)?;
        let project_dir = find_project_root(&current_dir)?.ok_or(Error::NotInitialized)?;
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
    /// [`Error::Config`] diagnostic naming the hub case rather than a
    /// stray `CapabilityResolution` lower down the stack.
    pub fn load_pipeline(&self) -> Result<PipelineView, Error> {
        let Some(capability) = self.config.capability.as_deref() else {
            return Err(Error::Config(
                "this project has no capability declared (hub projects do not run \
                 phase pipelines); only `specify registry` and `specify change` \
                 verbs are supported on hubs"
                    .to_string(),
            ));
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

fn find_project_root(start_dir: &Path) -> Result<Option<PathBuf>, Error> {
    for candidate in start_dir.ancestors() {
        let config_path = ProjectConfig::config_path(candidate);
        match config_path.try_exists() {
            Ok(true) => return Ok(Some(candidate.to_path_buf())),
            Ok(false) => {}
            Err(err) => return Err(Error::Io(err)),
        }
    }
    Ok(None)
}
