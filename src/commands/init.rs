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
    let context_skip_reason = generate_initial_context(format, &current_dir)?;
    emit_init_result(format, &result, context_skip_reason)
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
#[expect(
    clippy::struct_excessive_bools,
    reason = "JSON wire DTO: each bool is a stable, independently consumed field on the init envelope."
)]
struct Body {
    config_path: String,
    /// Resolved capability name (or `"hub"` for hub init — both
    /// renderers dispatch on this value).
    capability_name: String,
    cache_present: bool,
    directories_created: Vec<String>,
    scaffolded_rule_keys: Vec<String>,
    specify_version: String,
    /// `true` when this run scaffolded `.specify/wasm-pkg.toml`. Stays
    /// `false` on re-init so consumers can distinguish a fresh write
    /// from a preserved operator-edited file.
    wasm_pkg_config_written: bool,
    context_generated: bool,
    context_skipped: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    context_skip_reason: Option<&'static str>,
}

fn write_text(w: &mut dyn Write, body: &Body) -> std::io::Result<()> {
    let hub = body.capability_name == "hub";
    if hub {
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
    if body.wasm_pkg_config_written {
        writeln!(w, "  wrote .specify/wasm-pkg.toml (edit to add registry mappings)")?;
    }
    if body.context_skipped && body.context_skip_reason == Some("existing-agents-md") {
        writeln!(w, "AGENTS.md already present; skipping context generate")?;
    }
    writeln!(w)?;
    if hub {
        writeln!(
            w,
            "Next: run `specify registry add <id> <url>` to declare the projects this hub coordinates."
        )?;
    } else {
        writeln!(
            w,
            "Next: run `specify change draft <name> [--source <key>=<path-or-url> ...]` to scaffold the change brief and plan, then run `/change:draft` to author it."
        )?;
    }
    Ok(())
}

fn emit_init_result(
    format: Format, result: &InitResult, context_skip_reason: Option<&'static str>,
) -> Result<()> {
    let body = Body {
        config_path: canonical(&result.config_path),
        capability_name: result.capability_name.clone(),
        cache_present: result.cache_present,
        directories_created: result.directories_created.iter().map(|p| canonical(p)).collect(),
        scaffolded_rule_keys: result.scaffolded_rule_keys.clone(),
        specify_version: result.specify_version.clone(),
        wasm_pkg_config_written: result.wasm_pkg_config_written,
        context_generated: context_skip_reason.is_none(),
        context_skipped: context_skip_reason.is_some(),
        context_skip_reason,
    };
    output::emit(Box::new(std::io::stdout().lock()), format, &body, write_text)?;
    Ok(())
}

/// Returns `None` when initial context generation ran, `Some(reason)` when it was skipped.
fn generate_initial_context(format: Format, project_dir: &Path) -> Result<Option<&'static str>> {
    if is_workspace_clone(project_dir) {
        return Ok(Some("workspace-clone"));
    }
    match project_dir.join("AGENTS.md").try_exists() {
        Ok(true) => return Ok(Some("existing-agents-md")),
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
    Ok(None)
}
