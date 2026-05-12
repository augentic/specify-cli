//! Stub WASI runner used when the crate is built without the `host`
//! feature. Mirrors the public surface of [`crate::host`]; every
//! guest-execution path returns the `tool-host-not-built` diagnostic.

use std::path::PathBuf;

use crate::error::ToolError;
use crate::resolver::ResolvedTool;

/// Stdio configuration for a tool run.
#[derive(Debug, Copy, Clone, Default, PartialEq, Eq)]
pub enum Stdio {
    /// Inherit stdin, stdout, and stderr from the host process.
    #[default]
    Inherit,
    /// Drop stdin and sink stdout/stderr.
    Null,
}

/// Host-side context for running a resolved tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunContext {
    /// Project root used for `$PROJECT_DIR` and permission-root checks.
    pub project_dir: PathBuf,
    /// Canonical or canonicalisable capability root for capability-scope tools.
    pub capability_dir: Option<PathBuf>,
    /// Arguments forwarded after `argv[0]`, which is always the tool name.
    pub args: Vec<String>,
    /// Stdio handling for the WASI context.
    pub stdio: Stdio,
}

impl RunContext {
    /// Construct a run context with inherited stdio.
    #[must_use]
    pub fn new(project_dir: impl Into<PathBuf>, args: Vec<String>) -> Self {
        Self {
            project_dir: project_dir.into(),
            capability_dir: None,
            args,
            stdio: Stdio::Inherit,
        }
    }

    /// Attach a capability root for capability-scope tools.
    #[must_use]
    pub fn with_capability_dir(mut self, capability_dir: impl Into<PathBuf>) -> Self {
        self.capability_dir = Some(capability_dir.into());
        self
    }

    /// Override stdio handling.
    #[must_use]
    pub const fn with_stdio(mut self, stdio: Stdio) -> Self {
        self.stdio = stdio;
        self
    }
}

/// Stub runner that mirrors `host::WasiRunner` but cannot execute components.
#[derive(Debug, Default)]
pub struct WasiRunner {
    _private: (),
}

impl WasiRunner {
    /// Construct the stub. Always succeeds; the missing-host diagnostic is
    /// deferred to [`Self::run`] so plan-time helpers still build.
    ///
    /// # Errors
    ///
    /// Never returns an error in this build, but mirrors the host signature.
    pub const fn new() -> Result<Self, ToolError> {
        Ok(Self { _private: () })
    }

    /// Reject the run with the `tool-host-not-built` diagnostic.
    ///
    /// # Errors
    ///
    /// Always returns [`ToolError::HostNotBuilt`].
    pub const fn run(&self, _resolved: &ResolvedTool, _ctx: &RunContext) -> Result<i32, ToolError> {
        Err(ToolError::HostNotBuilt)
    }
}
