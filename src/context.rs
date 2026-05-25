use std::io::Write;
use std::path::PathBuf;

use serde::Serialize;
use specify_domain::adapter::{ResolvedTargetAdapter, TargetAdapter};
use specify_domain::config::{Layout, ProjectConfig};
use specify_domain::init::adapter_name_from_value;
use specify_error::Error;

use crate::cli::Format;
use crate::output;

/// Shared context for every subcommand that operates inside an
/// initialised `.specify/` project. Created once at the top of each
/// command handler via [`Ctx::load`].
pub struct Ctx {
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
        let project_dir = ProjectConfig::find_root(&current_dir).ok_or(Error::NotInitialized)?;
        let config = ProjectConfig::load(&project_dir)?;
        Ok(Self {
            format,
            project_dir,
            config,
        })
    }

    /// Resolve this project's target adapter into a
    /// [`ResolvedTargetAdapter`].
    ///
    /// Hub projects (`hub: true`, `adapter:` omitted) do not declare
    /// an adapter, so this returns a `hub-no-adapter` diagnostic
    /// naming the hub case rather than a stray adapter-resolution
    /// error lower down the stack.
    pub(crate) fn resolve_target_adapter(&self) -> Result<ResolvedTargetAdapter, Error> {
        let Some(adapter_value) = self.config.adapter.as_deref() else {
            return Err(Error::Diag {
                code: "hub-no-adapter",
                detail: "this project has no adapter declared (hub projects do not run \
                         per-target operations); only `specrun registry` and `specrun plan` \
                         verbs are supported on hubs"
                    .to_string(),
            });
        };
        let name = adapter_name_from_value(adapter_value);
        TargetAdapter::resolve(name, &self.project_dir)
    }

    /// Typed view over `.specify/`-anchored paths. Hand this to
    /// [`specify_domain::config::with_state`] in handlers that mutate
    /// `plan.yaml` / `registry.yaml`.
    pub(crate) fn layout(&self) -> Layout<'_> {
        Layout::new(&self.project_dir)
    }

    pub(crate) fn slices_dir(&self) -> PathBuf {
        self.layout().slices_dir()
    }

    pub(crate) fn archive_dir(&self) -> PathBuf {
        self.layout().archive_dir()
    }

    /// Serialise `body` and write it to stdout in this `Ctx`'s
    /// format, using `render_text` for the text-format branch. The
    /// text rendering is a free function colocated with the handler,
    /// so the response shape stays in a single block of code.
    ///
    /// # Errors
    ///
    /// Propagates the underlying serialization or I/O error from
    /// [`output::emit`].
    pub(crate) fn write<T: Serialize>(
        &self, body: &T, render_text: impl FnOnce(&mut dyn Write, &T) -> std::io::Result<()>,
    ) -> Result<(), Error> {
        output::emit(Box::new(std::io::stdout().lock()), self.format, body, render_text)
    }
}
