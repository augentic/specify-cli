//! Top-level clap derive surface for the `specrun` binary. Owns the
//! umbrella types ([`Cli`], [`Commands`], [`Format`], [`SourceArg`],
//! [`SliceSourceArg`]) and re-exports the per-verb action enums.

use std::str::FromStr;

use clap::{Parser, Subcommand};
use clap_complete::Shell;
use specify_domain::evidence::ClaimKind;

pub use crate::output::Format;
use crate::runtime::commands::lint::cli::LintAction;
use crate::runtime::commands::plan::cli::PlanAction;
use crate::runtime::commands::registry::cli::RegistryAction;
use crate::runtime::commands::rules::cli::RulesAction;
use crate::runtime::commands::slice::cli::SliceAction;
use crate::runtime::commands::source::cli::SourceAction;
use crate::runtime::commands::target::cli::TargetAction;
use crate::runtime::commands::tool::cli::ToolAction;
use crate::runtime::commands::workspace::cli::WorkspaceAction;

#[derive(Parser)]
#[command(
    name = "specrun",
    version,
    about = "Deterministic primitives for spec-driven development"
)]
pub struct Cli {
    #[command(subcommand)]
    pub(crate) command: Commands,

    /// Output format. `text` by default; pass `--format json` (or set
    /// `SPECRUN_FORMAT=json`) for structured envelopes when shelling
    /// out from skills.
    #[arg(long, env = "SPECRUN_FORMAT", default_value = "text", global = true)]
    pub(crate) format: Format,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Initialize .specify/ in a project.
    ///
    /// Pass `<adapter>` (bare name or URL) for a regular project, or
    /// `--hub` for a registry-only platform hub. The two are mutually
    /// exclusive — clap enforces the `<adapter>` xor `--hub` shape
    /// and exits `2` with its standard parse-error diagnostic when the
    /// invariant is violated.
    Init {
        /// Adapter identifier or URL (e.g. `omnia`,
        /// `https://github.com/<owner>/<repo>/adapters/targets/<name>`).
        /// Required unless `--hub` is set; mutually exclusive with `--hub`.
        #[arg(conflicts_with = "hub", required_unless_present = "hub")]
        adapter: Option<String>,
        /// Project name (defaults to the project directory name)
        #[arg(long)]
        name: Option<String>,
        /// Project domain description (tech stack, architecture, testing)
        #[arg(long)]
        domain: Option<String>,
        /// Scaffold a registry-only platform hub instead of a regular
        /// project. Refuses to run when `.specify/` already exists.
        #[arg(long)]
        hub: bool,
    },

    /// Source adapter operations (workflow contract). Source adapters provide
    /// `enumerate` + `extract` capabilities and are resolved against
    /// `adapters/sources/<name>/adapter.yaml` (in-repo) or
    /// `.specify/.cache/manifests/sources/<name>/` (agent manifest cache).
    Source {
        #[command(subcommand)]
        action: SourceAction,
    },

    /// Target adapter operations (workflow contract). Target adapters provide
    /// `shape` + `build` + `merge` capabilities and are resolved
    /// against `adapters/targets/<name>/adapter.yaml` (in-repo) or
    /// `.specify/.cache/manifests/targets/<name>/` (agent manifest cache).
    Target {
        #[command(subcommand)]
        action: TargetAction,
    },

    /// Rules resolution operations. Read-only: no
    /// `.specify/` writes, no journal events. Today the only verb is
    /// `export`, which streams a `ResolvedRules` JSON envelope built
    /// from the shared / source / target codex overlay tree.
    Rules {
        #[command(subcommand)]
        action: RulesAction,
    },

    /// WASI tool runner.
    Tool {
        #[command(subcommand)]
        action: ToolAction,
    },

    /// Deterministic lint (`specrun lint` v1). Resolves applicable codex
    /// rules, builds a `WorkspaceModel`, evaluates deterministic hints,
    /// and emits the `LintResult` envelope lint-result envelope. Read-only.
    Lint {
        #[command(subcommand)]
        action: LintAction,
    },

    /// Slice lifecycle operations — one `refine → build → merge` loop.
    Slice {
        #[command(subcommand)]
        action: SliceAction,
    },

    /// Executable plan operations — `plan.yaml` lifecycle and the
    /// `/spec:execute` driver lock.
    Plan {
        #[command(subcommand)]
        action: PlanAction,
    },

    /// Platform registry at `registry.yaml` (repo root)
    Registry {
        #[command(subcommand)]
        action: RegistryAction,
    },

    /// Materialise and manage registry peers under `.specify/workspace/`.
    Workspace {
        #[command(subcommand)]
        action: WorkspaceAction,
    },

    /// Print a shell-completion script for `<shell>` to stdout.
    ///
    /// Pipe into your shell's completion directory (e.g.
    /// `specrun completions zsh > ~/.zsh/_specrun`). Generated via
    /// `clap_complete`; the output tracks the live clap surface so
    /// every new verb is auto-discovered.
    Completions {
        /// Target shell — one of `bash`, `elvish`, `fish`, `powershell`, `zsh`.
        shell: Shell,
    },
}

/// Typed `--source <key>=<adapter>:<binding>` CLI value (top-level
/// plan source binding).
///
/// Wire grammar (locked at Specify 2.0):
///
/// - `--source <key>=<adapter>:<path>` — path-bound binding. The
///   adapter is the substring up to the first `:` after `=`; the
///   path is everything after that first `:` (URLs containing
///   `:` such as `git@github.com:org/foo.git` round-trip cleanly).
/// - `--source <key>=<adapter>:value:<literal>` — value-bound
///   binding. The `value:` sentinel after the adapter switches the
///   parser to literal mode; the literal payload is everything
///   after the second `:` and may contain anything (newlines,
///   colons, equals signs).
///
/// Materialises as [`specify_domain::change::SourceBinding`] under
/// the structured `{ adapter, path?, value? }` wire form. The 1.x
/// bare-string `--source <key>=<path>` form was dropped at the 2.0
/// cut — every binding now carries an explicit adapter name.
///
/// The [`FromStr`] impl returns a `String` error on malformed input
/// so clap surfaces a standard usage diagnostic (exit code 2).
#[derive(Clone)]
pub struct SourceArg {
    /// Source key (left of `=`).
    pub(crate) key: String,
    /// Kebab-case source-adapter name (parsed out of the `<adapter>:…`
    /// prefix after `=`).
    pub(crate) adapter: String,
    /// Mutually exclusive with `value`. `Some(path)` for the
    /// `<adapter>:<path>` form.
    pub(crate) path: Option<String>,
    /// Mutually exclusive with `path`. `Some(literal)` for the
    /// `<adapter>:value:<literal>` form.
    pub(crate) value: Option<String>,
}

impl FromStr for SourceArg {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (key, rest) = s.split_once('=').ok_or_else(|| {
            format!(
                "--source must be <key>=<adapter>:<path> or <key>=<adapter>:value:<literal>, got \
                 `{s}`"
            )
        })?;
        if key.is_empty() {
            return Err(format!("--source key must be non-empty, got `{s}`"));
        }
        let (adapter, body) = rest.split_once(':').ok_or_else(|| {
            format!(
                "--source value must be <adapter>:<path> or <adapter>:value:<literal>, got \
                 `{rest}` for key `{key}`"
            )
        })?;
        if adapter.is_empty() {
            return Err(format!("--source adapter must be non-empty, got `{s}`"));
        }
        if body.is_empty() {
            return Err(format!(
                "--source binding (path or `value:<literal>`) must be non-empty, got `{s}`"
            ));
        }
        let (path, value) = if let Some(literal) = body.strip_prefix("value:") {
            if literal.is_empty() {
                return Err(format!(
                    "--source value-literal must be non-empty after `value:`, got `{s}`"
                ));
            }
            (None, Some(literal.to_string()))
        } else {
            (Some(body.to_string()), None)
        };
        Ok(Self {
            key: key.to_string(),
            adapter: adapter.to_string(),
            path,
            value,
        })
    }
}

/// Typed value for the per-slice `--sources` / `--add-source` /
/// `--remove-source` flags.
///
/// Wire forms (workflow §`Slice.sources`):
///
/// - `<key>=<candidate-id>` — structured binding; both sides are
///   non-empty kebab identifiers. Materialises via
///   [`specify_domain::change::SliceSourceBinding::structured`].
/// - `<key>` — bare-string shorthand; sugar for
///   `{ key: <key>, candidate: <slice.name> }`. Materialises via
///   [`specify_domain::change::SliceSourceBinding::bare`].
///
/// Malformed inputs (empty key, empty candidate, dangling `=`, more
/// than one `=`) produce a `FromStr` error that clap surfaces as a
/// standard usage diagnostic (exit code 2 via `Error::Argument` at
/// the handler boundary).
#[derive(Clone, Debug)]
pub struct SliceSourceArg {
    pub(crate) key: String,
    /// `None` when the operator wrote the bare-string shorthand;
    /// `Some(candidate)` otherwise. The handler downconverts to the
    /// bare wire form when `candidate == slice.name` so the on-disk
    /// `plan.yaml` stays minimal.
    pub(crate) candidate: Option<String>,
}

/// Typed value for the per-slice `--authority-override <kind>=<key>`
/// flag on `specrun plan add` (where the slice context is implicit
/// from the command's positional `name`).
///
/// Wire form is `<claim-kind>=<source-key>`; both sides must be
/// non-empty and kebab-case (`source-key` is validated at the
/// `specrun slice validate` stage via the orphan-key check).
/// `claim-kind` is parsed at the CLI boundary against the closed
/// [`ClaimKind`] enum so misspellings fail before any plan mutation
/// runs (clap exits 2 with its standard usage diagnostic).
#[derive(Clone, Debug)]
pub struct AuthorityOverrideKindAssign {
    pub(crate) kind: ClaimKind,
    pub(crate) source_key: String,
}

/// Typed value for `specrun plan amend --add-alias` /
/// `--remove-alias` (discovery alias contract). Wire form is
/// `<candidate-id>=<alias>`; both sides must be non-empty
/// kebab-case strings. The closed [`specify_error::is_kebab`]
/// check runs at the handler boundary so the parser stays focused
/// on the `=` split.
#[derive(Clone, Debug)]
pub struct AliasAssign {
    /// Candidate id (left of `=`). The candidate must exist in
    /// `discovery.md`; the handler refuses with
    /// `discovery-candidate-unknown` otherwise.
    pub(crate) candidate: String,
    /// Alias value (right of `=`).
    pub(crate) alias: String,
}

impl FromStr for AliasAssign {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (candidate, alias) = s
            .split_once('=')
            .ok_or_else(|| format!("alias flag must be <candidate-id>=<alias>, got `{s}`"))?;
        if candidate.is_empty() || alias.is_empty() {
            return Err(format!(
                "alias flag candidate and alias must both be non-empty, got `{s}`"
            ));
        }
        if alias.contains('=') {
            return Err(format!("alias flag value `{s}` must contain exactly one `=` separator"));
        }
        Ok(Self {
            candidate: candidate.to_string(),
            alias: alias.to_string(),
        })
    }
}

impl FromStr for AuthorityOverrideKindAssign {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (raw_kind, source_key) = s.split_once('=').ok_or_else(|| {
            format!("--authority-override must be <kind>=<source-key>, got `{s}`")
        })?;
        if raw_kind.is_empty() || source_key.is_empty() {
            return Err(format!(
                "--authority-override kind and source-key must both be non-empty, got `{s}`"
            ));
        }
        if source_key.contains('=') {
            return Err(format!(
                "--authority-override value `{s}` must contain exactly one `=` separator between \
                 kind and source-key"
            ));
        }
        let kind: ClaimKind = raw_kind.parse()?;
        Ok(Self {
            kind,
            source_key: source_key.to_string(),
        })
    }
}

impl FromStr for SliceSourceArg {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            return Err("--sources value must be non-empty".to_string());
        }
        if let Some((k, v)) = s.split_once('=') {
            if v.contains('=') {
                return Err(format!(
                    "--sources value `{s}` must be <key>=<candidate-id> with at most one `=`"
                ));
            }
            if k.is_empty() || v.is_empty() {
                return Err(format!(
                    "--sources key and candidate-id must both be non-empty, got `{s}`"
                ));
            }
            Ok(Self {
                key: k.to_string(),
                candidate: Some(v.to_string()),
            })
        } else {
            Ok(Self {
                key: s.to_string(),
                candidate: None,
            })
        }
    }
}
