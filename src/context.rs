use std::path::PathBuf;

use specify_capability::PipelineView;
use specify_config::{Layout, LayoutExt, ProjectConfig};
use specify_error::Error;

use crate::cli::Format;
use crate::output::Out;

/// Shared context for every subcommand that operates inside an
/// initialised `.specify/` project. Created once at the top of each
/// command handler via [`Ctx::load`].
pub(crate) struct Ctx {
    pub(crate) format: Format,
    pub(crate) project_dir: PathBuf,
    pub(crate) config: ProjectConfig,
}

impl Ctx {
    /// Resolve the current project root, load `.specify/project.yaml`,
    /// and bundle everything into a `Ctx`.
    ///
    /// Returns `Err(Error)` on failure so callers can propagate with `?`.
    /// The top-level dispatcher (`scoped`) converts `Error` to
    /// the format-aware exit code.
    pub(crate) fn load(format: Format) -> Result<Self, Error> {
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
    pub(crate) fn load_pipeline(&self) -> Result<PipelineView, Error> {
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

    /// Typed view over `.specify/`-anchored paths. Hand this to
    /// [`specify_config::with_state`] /
    /// [`specify_config::with_existing_state`] in handlers that mutate
    /// `plan.yaml` / `registry.yaml`.
    pub(crate) fn layout(&self) -> Layout<'_> {
        self.project_dir.layout()
    }

    pub(crate) fn slices_dir(&self) -> PathBuf {
        self.project_dir.layout().slices_dir()
    }

    pub(crate) fn archive_dir(&self) -> PathBuf {
        self.project_dir.layout().archive_dir()
    }

    /// Stdout sink for handler success bodies. Use
    /// `ctx.out().write(&Body::from(&result))?;` so the
    /// `Stream::Stdout` constant stays inside `src/output.rs`.
    pub(crate) const fn out(&self) -> Out {
        Out::for_format(self.format)
    }
}
