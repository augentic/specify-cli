use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use serde::Serialize;
use specify_domain::config::{ProjectConfig, is_workspace_clone};
use specify_domain::init::{InitOptions, InitResult, VersionMode, init};
use specify_error::{Error, Result};

use crate::cli::Format;
use crate::commands::context;
use crate::context::Ctx;
use crate::output;

/// Display a path as the canonical absolute form when it exists; fall back
/// to the lossy display when it does not (e.g. a path we just deleted).
fn canonical(p: &Path) -> String {
    std::fs::canonicalize(p).map_or_else(|_| p.display().to_string(), |c| c.display().to_string())
}

pub(super) fn run(
    format: Format, capability: Option<&str>, name: Option<&str>, domain: Option<&str>, hub: bool,
) -> Result<()> {
    let project_dir = PathBuf::from(".");

    debug_assert!(
        hub != capability.is_some(),
        "clap enforces <capability> xor --hub; reached dispatcher with hub={hub}, capability={capability:?}",
    );

    let opts = InitOptions {
        project_dir: &project_dir,
        capability,
        name,
        domain,
        version_mode: VersionMode::WriteCurrent,
        hub,
    };

    let result = init(opts, Timestamp::now())?;
    let current_dir = std::env::current_dir().map_err(Error::Io)?;
    let context_generation = generate_initial_context(format, &current_dir)?;
    emit_init_result(format, &result, hub, context_generation)
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct Body {
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
    context: ContextBody,
}

fn write_text(w: &mut dyn Write, body: &Body) -> std::io::Result<()> {
    if body.hub {
        writeln!(w, "Initialized .specify/ as a registry-only platform hub")?;
    } else {
        writeln!(w, "Initialized .specify/")?;
    }
    writeln!(w, "  capability: {}", body.capability_name)?;
    writeln!(w, "  config: {}", body.config_path)?;
    writeln!(w, "  cache present: {}", body.cache_present)?;
    if !body.directories_created.is_empty() {
        writeln!(w, "  directories created: {}", body.directories_created.join(", "))?;
    }
    writeln!(w, "  specify_version: {}", body.specify_version)?;
    if body.context.skipped && body.context.skip_reason == Some("existing-agents-md") {
        writeln!(w, "AGENTS.md already present; skipping context generate")?;
    }
    writeln!(w)?;
    if body.hub {
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

#[derive(Serialize)]
struct ContextBody {
    #[serde(rename = "context-generated")]
    generated: bool,
    #[serde(rename = "context-skipped")]
    skipped: bool,
    #[serde(rename = "context-skip-reason", skip_serializing_if = "Option::is_none")]
    skip_reason: Option<&'static str>,
}

impl From<ContextGeneration> for ContextBody {
    fn from(context_generation: ContextGeneration) -> Self {
        Self {
            generated: matches!(context_generation, ContextGeneration::Generated),
            skipped: context_generation.skipped(),
            skip_reason: context_generation.skip_reason(),
        }
    }
}

fn emit_init_result(
    format: Format, result: &InitResult, hub: bool, context_generation: ContextGeneration,
) -> Result<()> {
    let body = Body {
        config_path: canonical(&result.config_path),
        capability_name: result.capability_name.clone(),
        cache_present: result.cache_present,
        directories_created: result.directories_created.iter().map(|p| canonical(p)).collect(),
        scaffolded_rule_keys: result.scaffolded_rule_keys.clone(),
        specify_version: result.specify_version.clone(),
        hub,
        context: ContextBody::from(context_generation),
    };
    output::write(format, &body, write_text)?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContextGeneration {
    Generated,
    Skipped { reason: &'static str },
}

impl ContextGeneration {
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

fn generate_initial_context(format: Format, project_dir: &Path) -> Result<ContextGeneration> {
    if is_workspace_clone(project_dir) {
        return Ok(ContextGeneration::Skipped {
            reason: "workspace-clone",
        });
    }
    match project_dir.join("AGENTS.md").try_exists() {
        Ok(true) => {
            return Ok(ContextGeneration::Skipped {
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
    Ok(ContextGeneration::Generated)
}
