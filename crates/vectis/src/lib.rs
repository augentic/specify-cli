//! `specify-vectis` library: handlers and arg types for the vectis subcommands.
//!
//! Chunks 1–2 carve the original standalone `vectis` binary out into a
//! library so the `specify` binary can dispatch through to the same
//! handlers (chunk 3 wires that dispatcher up). Each subcommand handler
//! returns `Result<CommandOutcome, VectisError>`; the dispatcher decides
//! how to render the success payload, the `Stub { command }` placeholder,
//! and the error JSON envelope (chunk 4 finalises the kebab-case
//! v2 contract).
//!
//! The arg structs below remain `clap::Args` so a host clap derive (the
//! `specify` `Cli`) can `#[command(flatten)]`/embed them as subcommand
//! payloads without redefining their flags.

pub mod add_shell;
pub mod error;
pub mod init;
pub mod update_versions;
pub mod verify;
pub mod versions_cmd;

mod prerequisites;
mod templates;
pub mod versions;

pub use error::{MissingTool, VectisError};
pub use versions::Versions;

/// Outcome returned by every subcommand handler.
///
/// `Success` carries the handler's normal JSON output (the dispatcher
/// prints it and exits 0). `Stub` is a placeholder for handlers that
/// have not been implemented yet; the dispatcher prints the RFC's
/// `not-implemented` shape and exits non-zero. Real failures flow
/// through `Err(VectisError)`.
#[derive(Debug)]
#[non_exhaustive]
pub enum CommandOutcome {
    /// Handler completed normally with a JSON payload.
    Success(serde_json::Value),
    /// Handler is not yet implemented.
    Stub {
        /// The subcommand name that produced this stub.
        command: &'static str,
    },
}

/// `vectis init` arguments.
///
/// Fields below are populated by clap. `InitArgs` is fully consumed by
/// chunk 5 (`app_name`, `dir`, `version_file`, `android_package` -- the
/// last as the source of the default Android package even for core-only
/// scaffolds, since `__ANDROID_PACKAGE__` lives in `codegen.rs`) and
/// chunk 6 (`caps`). See rfcs/rfc-6-tasks.md.
#[derive(clap::Args, Debug)]
pub struct InitArgs {
    /// App struct name (`PascalCase`, e.g. "Counter", "`TodoApp`").
    pub app_name: String,

    /// Project directory (defaults to current directory).
    #[arg(long)]
    pub dir: Option<std::path::PathBuf>,

    /// Comma-separated capabilities. Values: http, kv, time, platform, sse.
    #[arg(long)]
    pub caps: Option<String>,

    /// Comma-separated shell platforms. Values: ios, android.
    #[arg(long)]
    pub shells: Option<String>,

    /// Android package name (defaults to `com.vectis.<appname>` lowercase).
    #[arg(long)]
    pub android_package: Option<String>,

    /// Override version pins file. When set, the file MUST exist; resolution
    /// otherwise falls back to `<project>/versions.toml`,
    /// `~/.config/vectis/versions.toml`, then the embedded defaults.
    #[arg(long)]
    pub version_file: Option<std::path::PathBuf>,
}

/// `vectis verify` arguments.
#[derive(clap::Args, Debug)]
pub struct VerifyArgs {
    /// Project directory (defaults to current directory).
    #[arg(long)]
    pub dir: Option<std::path::PathBuf>,

    /// Override version pins file. When set, the file MUST exist; see
    /// `vectis init --help` for the full resolution order.
    #[arg(long)]
    pub version_file: Option<std::path::PathBuf>,
}

/// `vectis add-shell` arguments.
#[derive(clap::Args, Debug)]
pub struct AddShellArgs {
    /// Shell platform to add. Values: ios, android.
    pub platform: String,

    /// Project directory (defaults to current directory).
    #[arg(long)]
    pub dir: Option<std::path::PathBuf>,

    /// Android package name (defaults to `com.vectis.<appname>` lowercase).
    #[arg(long)]
    pub android_package: Option<String>,

    /// Override version pins file. When set, the file MUST exist; see
    /// `vectis init --help` for the full resolution order.
    #[arg(long)]
    pub version_file: Option<std::path::PathBuf>,
}

/// `vectis update-versions` arguments.
#[derive(clap::Args, Debug)]
pub struct UpdateVersionsArgs {
    /// File to update (defaults to ~/.config/vectis/versions.toml). For
    /// `update-versions` this is the *write target*, not a resolution
    /// override -- on the other subcommands the same flag overrides
    /// resolution.
    #[arg(long)]
    pub version_file: Option<std::path::PathBuf>,

    /// Show proposed changes without writing.
    #[arg(long)]
    pub dry_run: bool,

    /// Scaffold a scratch project and run `vectis verify` before committing pins.
    #[arg(long)]
    pub verify: bool,
}

/// `vectis versions` arguments.
#[derive(clap::Args, Debug)]
pub struct VersionsArgs {
    /// Project directory (defaults to current directory).
    #[arg(long)]
    pub dir: Option<std::path::PathBuf>,

    /// Override version pins file. When set, the file MUST exist; see
    /// `vectis init --help` for the full resolution order.
    #[arg(long)]
    pub version_file: Option<std::path::PathBuf>,
}
