//! Render-only implementation for the `vectis scaffold` subcommand.
//!
//! The module accepts only explicit CLI inputs and the `PROJECT_DIR`
//! declared by the WASI host. It renders embedded templates, plans
//! every target file, then refuses all overwrites before creating
//! directories or writing bytes. Per-target planning, the on-disk
//! write step, and `app_name` validation live in the private `runtime`
//! submodule; this
//! parent module owns the clap derive surface, the public DTOs, and
//! the dispatch path.

mod runtime;
mod templates;
#[cfg(test)]
mod tests;
mod versions;

use std::path::{Path, PathBuf};

use clap::{Args as ClapArgs, Subcommand};
pub use runtime::{plan_android, plan_core, plan_ios, validate_app_name, write_plan};
pub use templates::Capability;
pub use versions::Versions;

/// Compatibility alias for the unified crate-wide error type.
///
/// Scaffold-side callers (and their tests) historically referred to
/// `ScaffoldError`; the type itself now lives at the crate root.
pub use crate::VectisError as ScaffoldError;
use crate::render_json as render_value;

/// Scaffold targets exposed under `vectis scaffold`.
#[derive(Subcommand, Debug, Clone, PartialEq, Eq)]
pub enum ScaffoldCommand {
    /// Render the shared Rust Crux core crate.
    Core(CoreArgs),
    /// Render the `SwiftUI` iOS shell.
    Ios(IosArgs),
    /// Render the Jetpack Compose Android shell.
    Android(AndroidArgs),
}

impl ScaffoldCommand {
    /// Return the stable CLI spelling for this scaffold target.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Core(_) => "core",
            Self::Ios(_) => "ios",
            Self::Android(_) => "android",
        }
    }

    /// Return the app name supplied to this scaffold target.
    #[must_use]
    pub fn app_name(&self) -> &str {
        match self {
            Self::Core(args) => &args.common.app_name,
            Self::Ios(args) => &args.common.app_name,
            Self::Android(args) => &args.common.app_name,
        }
    }

    /// Return common arguments for this command.
    #[must_use]
    pub const fn common(&self) -> &CommonArgs {
        match self {
            Self::Core(args) => &args.common,
            Self::Ios(args) => &args.common,
            Self::Android(args) => &args.common,
        }
    }
}

/// Arguments for `vectis scaffold core`.
#[derive(ClapArgs, Debug, Clone, PartialEq, Eq)]
pub struct CoreArgs {
    /// Common app, capability, and version arguments.
    #[command(flatten)]
    pub common: CommonArgs,

    /// Android package name used when rendering Android-facing core bindings.
    #[arg(long)]
    pub android_package: Option<String>,
}

/// Arguments for `vectis scaffold ios`.
#[derive(ClapArgs, Debug, Clone, PartialEq, Eq)]
pub struct IosArgs {
    /// Common app, capability, and version arguments.
    #[command(flatten)]
    pub common: CommonArgs,
}

/// Arguments for `vectis scaffold android`.
#[derive(ClapArgs, Debug, Clone, PartialEq, Eq)]
pub struct AndroidArgs {
    /// Common app, capability, and version arguments.
    #[command(flatten)]
    pub common: CommonArgs,

    /// Android application package name.
    #[arg(long)]
    pub android_package: Option<String>,
}

/// Arguments shared by all scaffold targets.
#[derive(ClapArgs, Debug, Clone, PartialEq, Eq)]
pub struct CommonArgs {
    /// App struct/name to scaffold, for example `Counter` or `TodoApp`.
    pub app_name: String,

    /// Comma-separated capabilities, for example `http,kv,time`.
    #[arg(long)]
    pub caps: Option<String>,

    /// Complete TOML file overriding the embedded version defaults.
    #[arg(long)]
    pub version_file: Option<PathBuf>,
}

/// A rendered file ready to write under `PROJECT_DIR`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedFile {
    /// Relative target path under `PROJECT_DIR`.
    pub relative_path: String,
    /// Rendered file bytes as UTF-8 text.
    pub contents: String,
}

/// A complete scaffold plan for one target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScaffoldPlan {
    /// Scaffold target name.
    pub target: &'static str,
    /// App name supplied by the caller.
    pub app_name: String,
    /// Android package used for placeholders.
    pub android_package: String,
    /// Selected capability tags in stable input order.
    pub capabilities: Vec<String>,
    /// Files to write in template declaration order.
    pub files: Vec<PlannedFile>,
}

impl ScaffoldPlan {
    fn file_paths(&self) -> Vec<String> {
        self.files.iter().map(|file| file.relative_path.clone()).collect()
    }

    fn to_json(&self, project_dir: &Path) -> serde_json::Value {
        serde_json::json!({
            "target": self.target,
            "app-name": self.app_name,
            "project-dir": project_dir.display().to_string(),
            "android-package": self.android_package,
            "capabilities": self.capabilities,
            "files": self.file_paths(),
        })
    }
}

/// Execute a parsed scaffold subcommand.
///
/// # Errors
///
/// Returns [`ScaffoldError`] for invalid inputs, version-file issues, missing
/// `PROJECT_DIR`, or write failures.
pub fn run(command: &ScaffoldCommand) -> Result<serde_json::Value, ScaffoldError> {
    let project_dir = project_dir_from_env()?;
    let versions = Versions::resolve(command.common().version_file.as_deref())?;
    let plan = plan_command(command, &versions)?;
    write_plan(&project_dir, &plan)?;
    Ok(plan.to_json(&project_dir))
}

/// Plan a scaffold command without touching the filesystem.
///
/// # Errors
///
/// Returns [`ScaffoldError`] when arguments are invalid.
pub fn plan_command(
    command: &ScaffoldCommand, versions: &Versions,
) -> Result<ScaffoldPlan, ScaffoldError> {
    match command {
        ScaffoldCommand::Core(args) => {
            let caps = parse_caps(args.common.caps.as_deref())?;
            let android_package = args
                .android_package
                .clone()
                .unwrap_or_else(|| default_android_package(&args.common.app_name));
            plan_core(&args.common.app_name, &android_package, &caps, versions)
        }
        ScaffoldCommand::Ios(args) => {
            let caps = parse_caps(args.common.caps.as_deref())?;
            let android_package = default_android_package(&args.common.app_name);
            plan_ios(&args.common.app_name, &android_package, &caps, versions)
        }
        ScaffoldCommand::Android(args) => {
            let caps = parse_caps(args.common.caps.as_deref())?;
            let android_package = args
                .android_package
                .clone()
                .unwrap_or_else(|| default_android_package(&args.common.app_name));
            plan_android(&args.common.app_name, &android_package, &caps, versions)
        }
    }
}

/// Compute the default Android package: `com.vectis.<lower app name>`.
#[must_use]
pub fn default_android_package(app_name: &str) -> String {
    format!("com.vectis.{}", app_name.to_lowercase())
}

/// Parse the `--caps` flag into the canonical capability set.
///
/// # Errors
///
/// Returns [`ScaffoldError`] when an unknown capability tag is present.
pub fn parse_caps(raw: Option<&str>) -> Result<Vec<Capability>, ScaffoldError> {
    let mut out: Vec<Capability> = Vec::new();
    let Some(raw) = raw else { return Ok(out) };
    for tag in raw.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let cap = Capability::from_tag(tag).ok_or_else(|| ScaffoldError::InvalidProject {
            message: format!(
                "unknown capability: {tag:?} (expected one of: http, kv, time, platform, sse)"
            ),
        })?;
        if !out.contains(&cap) {
            out.push(cap);
        }
    }
    Ok(out)
}

/// Render a `(success | error)` result as pretty-printed JSON.
#[must_use]
pub fn render_json(outcome: Result<serde_json::Value, ScaffoldError>) -> (String, u8) {
    match outcome {
        Ok(value) => (render_value(&value), 0),
        Err(err) => {
            let exit_code = err.exit_code();
            let serde_json::Value::Object(mut payload) = err.to_json() else {
                unreachable!("ScaffoldError::to_json always returns an object")
            };
            payload.entry("exit-code".to_string()).or_insert(serde_json::Value::from(exit_code));
            (render_value(&serde_json::Value::Object(payload)), exit_code)
        }
    }
}

fn project_dir_from_env() -> Result<PathBuf, ScaffoldError> {
    std::env::var_os("PROJECT_DIR").map(PathBuf::from).ok_or_else(|| {
        ScaffoldError::InvalidProject {
            message: "PROJECT_DIR is not set; run through `specify tool run` with a project scope"
                .into(),
        }
    })
}
