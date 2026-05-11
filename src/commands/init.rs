use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::Serialize;
use specify_domain::config::{ProjectConfig, is_workspace_clone};
use specify_error::{Error, Result};
use specify_domain::init::{InitOptions, InitResult, VersionMode, init};

use crate::cli::Format;
use crate::commands::context;
use crate::context::Ctx;
use crate::output::{self, Render, display};

/// Dispatcher for `specify init`.
///
/// The `<capability>` xor `--hub` invariant is enforced by clap (see
/// `#[arg(conflicts_with = "hub", required_unless_present = "hub")]`
/// on the `capability` positional in `crate::cli::Commands::Init`),
/// so by the time this function runs the `(hub, capability)` pair is
/// guaranteed to be one of `(false, Some(_))` or `(true, None)`.
pub(super) fn run(
    format: Format, capability: Option<String>, name: Option<&str>, domain: Option<&str>, hub: bool,
) -> Result<()> {
    let project_dir = PathBuf::from(".");

    debug_assert!(
        hub != capability.is_some(),
        "clap enforces <capability> xor --hub; reached dispatcher with hub={hub}, capability={capability:?}",
    );

    let opts = InitOptions {
        project_dir: &project_dir,
        capability: capability.as_deref(),
        name,
        domain,
        version_mode: VersionMode::WriteCurrent,
        hub,
    };

    let result = init(opts, Utc::now())?;
    let current_dir = std::env::current_dir().map_err(Error::Io)?;
    let context_generation = generate_initial_context(format, &current_dir)?;
    emit_init_result(format, &result, hub, context_generation)
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct InitBody {
    config_path: String,
    /// Resolved capability name (or `"hub"` for hub init).
    capability_name: String,
    cache_present: bool,
    directories_created: Vec<String>,
    scaffolded_rule_keys: Vec<String>,
    specify_version: String,
    /// `true` when this init scaffolded a registry-only platform hub.
    /// Always present so consumers can distinguish hub from regular
    /// initialisations without parsing the capability name.
    hub: bool,
    #[serde(flatten)]
    context: InitContextBody,
}

impl Render for InitBody {
    fn render_text(&self, w: &mut dyn Write) -> std::io::Result<()> {
        if self.hub {
            writeln!(w, "Initialized .specify/ as a registry-only platform hub")?;
        } else {
            writeln!(w, "Initialized .specify/")?;
        }
        writeln!(w, "  capability: {}", self.capability_name)?;
        writeln!(w, "  config: {}", self.config_path)?;
        writeln!(w, "  cache present: {}", self.cache_present)?;
        if !self.directories_created.is_empty() {
            writeln!(w, "  directories created: {}", self.directories_created.join(", "))?;
        }
        writeln!(w, "  specify_version: {}", self.specify_version)?;
        if self.context.skipped && self.context.skip_reason == Some("existing-agents-md") {
            writeln!(w, "AGENTS.md already present; skipping context generate")?;
        }
        writeln!(w)?;
        if self.hub {
            writeln!(
                w,
                "Next: run `specify registry add <id> <url>` to declare the projects this hub coordinates."
            )?;
        } else {
            writeln!(
                w,
                "Next: run `specify change create <name>` to start a change, then `specify change plan create <name>` to plan it."
            )?;
        }
        Ok(())
    }
}

#[derive(Serialize)]
struct InitContextBody {
    #[serde(rename = "context-generated")]
    generated: bool,
    #[serde(rename = "context-skipped")]
    skipped: bool,
    #[serde(rename = "context-skip-reason", skip_serializing_if = "Option::is_none")]
    skip_reason: Option<&'static str>,
}

impl From<InitContextGeneration> for InitContextBody {
    fn from(context_generation: InitContextGeneration) -> Self {
        Self {
            generated: matches!(context_generation, InitContextGeneration::Generated),
            skipped: context_generation.skipped(),
            skip_reason: context_generation.skip_reason(),
        }
    }
}

fn emit_init_result(
    format: Format, result: &InitResult, hub: bool, context_generation: InitContextGeneration,
) -> Result<()> {
    let body = InitBody {
        config_path: display(&result.config_path),
        capability_name: result.capability_name.clone(),
        cache_present: result.cache_present,
        directories_created: result.directories_created.iter().map(|p| display(p)).collect(),
        scaffolded_rule_keys: result.scaffolded_rule_keys.clone(),
        specify_version: result.specify_version.clone(),
        hub,
        context: InitContextBody::from(context_generation),
    };
    output::write(format, &body)?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InitContextGeneration {
    Generated,
    Skipped { reason: &'static str },
}

impl InitContextGeneration {
    const fn skip_reason(&self) -> Option<&'static str> {
        match self {
            Self::Generated => None,
            Self::Skipped { reason } => Some(*reason),
        }
    }

    const fn skipped(&self) -> bool {
        self.skip_reason().is_some()
    }
}

fn generate_initial_context(format: Format, project_dir: &Path) -> Result<InitContextGeneration> {
    if is_workspace_clone(project_dir) {
        return Ok(InitContextGeneration::Skipped {
            reason: "workspace-clone",
        });
    }
    match project_dir.join("AGENTS.md").try_exists() {
        Ok(true) => {
            return Ok(InitContextGeneration::Skipped {
                reason: "existing-agents-md",
            });
        }
        Ok(false) => {}
        Err(err) if err.kind() == ErrorKind::NotFound => {}
        Err(err) => return Err(Error::Io(err)),
    }

    let config = ProjectConfig::load(project_dir)?;
    let ctx = Ctx {
        format,
        project_dir: project_dir.to_path_buf(),
        config,
    };
    let outcome = context::generate_for_init(&ctx)?;
    debug_assert!(
        outcome.changed,
        "init context generation is called only when AGENTS.md is absent"
    );
    debug_assert_eq!(outcome.disposition, "create");
    Ok(InitContextGeneration::Generated)
}
