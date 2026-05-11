use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;
use specify_config::{ProjectConfig, is_workspace_clone_path};
use specify_error::{Error, Result};
use specify_init::{InitOptions, InitResult, VersionMode, init};

use crate::cli::OutputFormat;
use crate::commands::context;
use crate::context::Ctx;
use crate::output::{Render, Stream, emit, path_string};

/// Dispatcher for `specify init`.
///
/// Enforces mutual exclusion between the `<capability>` positional and
/// `--hub`:
///
/// - regular project init requires `<capability>`;
/// - hub init requires `--hub` and refuses a `<capability>` positional;
/// - missing both, or both at once, errors with
///   `init-requires-capability-or-hub`.
pub fn run(
    format: OutputFormat, capability: Option<String>, name: Option<String>, domain: Option<String>,
    hub: bool,
) -> Result<()> {
    let project_dir = PathBuf::from(".");

    let capability = match (hub, capability) {
        (false, Some(cap)) => Some(cap),
        (true, None) => None,
        // Both unset, or both set: the diagnostic is the same — the
        // operator must pick one.
        (false, None) | (true, Some(_)) => return Err(Error::InitNeedsCapability),
    };

    let opts = InitOptions {
        project_dir: &project_dir,
        capability: capability.as_deref(),
        name: name.as_deref(),
        domain: domain.as_deref(),
        version_mode: VersionMode::WriteCurrent,
        hub,
    };

    let result = init(opts)?;
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
            generated: context_generation.generated(),
            skipped: context_generation.skipped(),
            skip_reason: context_generation.skip_reason(),
        }
    }
}

fn emit_init_result(
    format: OutputFormat, result: &InitResult, hub: bool, context_generation: InitContextGeneration,
) -> Result<()> {
    let body = InitBody {
        config_path: path_string(&result.config_path),
        capability_name: result.capability_name.clone(),
        cache_present: result.cache_present,
        directories_created: result.directories_created.iter().map(|p| path_string(p)).collect(),
        scaffolded_rule_keys: result.scaffolded_rule_keys.clone(),
        specify_version: result.specify_version.clone(),
        hub,
        context: InitContextBody::from(context_generation),
    };
    emit(Stream::Stdout, format, &body)?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InitContextGeneration {
    Generated,
    SkippedExistingAgents,
    SkippedWorkspaceClone,
}

impl InitContextGeneration {
    const fn generated(self) -> bool {
        matches!(self, Self::Generated)
    }

    const fn skipped(self) -> bool {
        matches!(self, Self::SkippedExistingAgents | Self::SkippedWorkspaceClone)
    }

    const fn skip_reason(self) -> Option<&'static str> {
        match self {
            Self::Generated => None,
            Self::SkippedExistingAgents => Some("existing-agents-md"),
            Self::SkippedWorkspaceClone => Some("workspace-clone"),
        }
    }
}

fn generate_initial_context(
    format: OutputFormat, project_dir: &Path,
) -> Result<InitContextGeneration> {
    if is_workspace_clone_path(project_dir) {
        return Ok(InitContextGeneration::SkippedWorkspaceClone);
    }
    match project_dir.join("AGENTS.md").try_exists() {
        Ok(true) => return Ok(InitContextGeneration::SkippedExistingAgents),
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
