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
pub mod validate;
pub mod verify;
pub mod versions_cmd;

mod prerequisites;
mod templates;
pub mod versions;

pub use error::{MissingTool, VectisError};
pub use versions::Versions;

/// JSON contract version emitted on every structured response.
///
/// Bumping this is a breaking change for skill authors. Frozen at `2`
/// to match the v2 envelope the pre-RFC-13 `specify vectis *`
/// dispatcher produced (kebab-case keys, auto-injected `schema-version`,
/// kebab-case error variants — see RFC-2 §2 and the `specify` binary's
/// `output::JSON_SCHEMA_VERSION`).
pub const JSON_SCHEMA_VERSION: u64 = 2;

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

/// Render a `(CommandOutcome | VectisError)` as the v2 JSON envelope
/// (`schema-version: 2` + payload), pretty-printed.
///
/// Both the standalone `specify-vectis` binary (RFC-13 §4.3a) and any
/// capability skill that forwards Vectis output verbatim should funnel
/// through this helper. The byte sequence matches the legacy
/// `specify vectis * --format json` output that pre-2.6 operators
/// scripted against:
///
/// * **Success** — `{"schema-version": 2, ...payload object fields...}`.
///   Fields appear in the order the handler's `serde_json::json!` macro
///   inserted them, so any future field-order change in the handlers is
///   visible to the parity tests rather than masked by this helper.
/// * **Stub** — `{"schema-version": 2, "error": "not-implemented",
///   "command": "<verb>", "message": "...", "exit-code": 1}`.
/// * **Err(VectisError)** — `{"schema-version": 2, ...VectisError::to_json
///   payload..., "exit-code": <variant code>}`. The variant-specific keys
///   (`missing` for `MissingPrerequisites`, `message` everywhere) come
///   straight from [`VectisError::to_json`] so this helper cannot drift
///   from the typed error surface.
///
/// Returns the envelope JSON string (without trailing newline; the bin
/// adds one via `println!`) and the exit code the caller should use.
///
/// # Panics
///
/// Panics if [`VectisError::to_json`] ever returns a non-object value,
/// which the type contract forbids.
#[must_use]
pub fn render_envelope_json(outcome: Result<CommandOutcome, VectisError>) -> (String, u8) {
    match outcome {
        Ok(CommandOutcome::Success(value)) => (envelope_json(value), 0),
        Ok(CommandOutcome::Stub { command }) => {
            let payload = serde_json::json!({
                "error": "not-implemented",
                "command": command,
                "message": format!("`vectis {command}` is not implemented yet"),
                "exit-code": 1u8,
            });
            (envelope_json(payload), 1)
        }
        Err(err) => {
            let exit_code = u8::try_from(err.exit_code()).unwrap_or(1);
            let serde_json::Value::Object(mut payload) = err.to_json() else {
                unreachable!("VectisError::to_json always returns an object")
            };
            payload.entry("exit-code".to_string()).or_insert(serde_json::Value::from(exit_code));
            (envelope_json(serde_json::Value::Object(payload)), exit_code)
        }
    }
}

/// Pretty-print `payload` under the v2 envelope (`schema-version: 2`
/// first, then flattened payload fields). Internal helper for
/// [`render_envelope_json`].
fn envelope_json(payload: serde_json::Value) -> String {
    use serde::Serialize;

    #[derive(Serialize)]
    struct Envelope {
        #[serde(rename = "schema-version")]
        schema_version: u64,
        #[serde(flatten)]
        payload: serde_json::Value,
    }

    serde_json::to_string_pretty(&Envelope {
        schema_version: JSON_SCHEMA_VERSION,
        payload,
    })
    .expect("JSON serialise")
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

/// `vectis validate <mode> [path]` arguments (RFC-11 §H).
///
/// `mode` is required; `path` is optional. When `path` is omitted the
/// dispatcher resolves the canonical RFC-11 §H Vectis paths (slice-local
/// files first, then project-level design-system files or the merged
/// composition baseline). Legacy projects that still vendor a
/// `schema.yaml` `artifacts:` block can override those defaults. An
/// explicit `path` always wins.
///
/// All five modes (`layout`, `composition`, `tokens`, `assets`, `all`)
/// are fully implemented. Per-mode runs return
/// `CommandOutcome::Success` with a v2 JSON envelope containing
/// `mode`, `path`, `errors`, and `warnings`. The `all` mode returns
/// a `{ mode: "all", path, results }` envelope where each entry
/// wraps a per-mode `report`. The dispatcher exits zero when no
/// errors are found.
#[derive(clap::Args, Debug)]
pub struct ValidateArgs {
    /// Validation mode. Choose one of `layout`, `composition`,
    /// `tokens`, `assets`, or `all`.
    #[arg(value_enum)]
    pub mode: ValidateMode,

    /// Optional path to the artifact (or, for `all`, the project
    /// root). When omitted, defaults are resolved from the
    /// canonical Vectis path cascade (Phase 1.10) with an embedded
    /// fallback.
    pub path: Option<std::path::PathBuf>,
}

/// Validation mode discriminant for `vectis validate`.
///
/// Five-valued enum mirroring the RFC-11 §H verb list. Variant
/// spellings (kebab-case via `clap::ValueEnum`) match the strings the
/// `Stub { command }` payload threads through to the v2 `command`
/// field — keep them in lock-step with `validate::run`'s match arms.
#[derive(Copy, Clone, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum ValidateMode {
    /// Validate `layout.yaml` as the unwired subset of the composition
    /// schema (RFC-11 §H "`layout` mode").
    Layout,
    /// Validate `composition.yaml` as the lifecycle artifact, with
    /// auto-invoked tokens / assets cross-checks (RFC-11 §H
    /// "`composition` mode").
    Composition,
    /// Validate `tokens.yaml` against `tokens.schema.json` (RFC-11
    /// Appendix A).
    Tokens,
    /// Validate `assets.yaml` against `assets.schema.json` plus
    /// referenced-file existence checks (RFC-11 §E, Appendix B).
    Assets,
    /// Run every other mode against the active change + baseline and
    /// emit a combined report (RFC-11 §H "CLI validation modes"
    /// closing paragraph).
    All,
}

impl ValidateMode {
    /// Stable kebab-case name used in stub payloads, JSON output, and
    /// text rendering. Matches the CLI value-enum spelling exactly so
    /// `--format json` consumers and `--format text` operators see
    /// the same identifier.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Layout => "layout",
            Self::Composition => "composition",
            Self::Tokens => "tokens",
            Self::Assets => "assets",
            Self::All => "all",
        }
    }
}
