use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use serde::Serialize;
use specify_error::{Error, Result};
use specify_workflow::config::{ProjectConfig, is_workspace_clone};
use specify_workflow::init::{InitOptions, InitResult, init};

use crate::runtime::cli::Format;
use crate::runtime::commands::agents;
use crate::runtime::context::Ctx;
use crate::runtime::output;

/// Display a path as the canonical absolute form when it exists; fall back
/// to the lossy display when it does not (e.g. a path we just deleted).
fn canonical(p: &Path) -> String {
    std::fs::canonicalize(p).map_or_else(|_| p.display().to_string(), |c| c.display().to_string())
}

pub(super) fn run(
    format: Format, adapter: Option<&str>, name: Option<&str>, domain: Option<&str>, hub: bool,
    include_framework: bool,
) -> Result<()> {
    let project_dir = PathBuf::from(".");

    let opts = InitOptions {
        project_dir: &project_dir,
        adapter,
        name,
        domain,
        hub,
        include_framework,
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
    /// Resolved adapter name (or `"hub"` for hub init — both
    /// renderers dispatch on this value).
    adapter_name: String,
    cache_present: bool,
    /// `true` when the shared codex was distributed into
    /// `.specify/.cache/codex/` (RM-07). `false` when the adapter source
    /// carries no shared pack, and always `false` for hub init.
    codex_present: bool,
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
    let hub = body.adapter_name == "hub";
    if hub {
        writeln!(w, "Initialized .specify/ as a registry-only platform hub")?;
    } else {
        writeln!(w, "Initialized .specify/")?;
    }
    writeln!(w, "  adapter: {}", body.adapter_name)?;
    writeln!(w, "  config: {}", body.config_path)?;
    writeln!(w, "  cache present: {}", body.cache_present)?;
    if !hub {
        writeln!(w, "  codex present: {}", body.codex_present)?;
    }
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
            "Next: run `specrun registry add <id> <url>` to declare the projects this hub coordinates."
        )?;
    } else {
        writeln!(
            w,
            "Next: run `/spec:plan <name>` (the skill that authors `change.md` + `plan.yaml`), or — for a headless plan — `specrun plan create <name>` followed by `specrun plan add` and `specrun plan transition <name> approved`."
        )?;
    }
    Ok(())
}

fn emit_init_result(
    format: Format, result: &InitResult, context_skip_reason: Option<&'static str>,
) -> Result<()> {
    let body = Body {
        config_path: canonical(&result.config_path),
        adapter_name: result.adapter_name.clone(),
        cache_present: result.cache_present,
        codex_present: result.codex_present,
        directories_created: result.directories_created.iter().map(|p| canonical(p)).collect(),
        scaffolded_rule_keys: result.scaffolded_rule_keys.clone(),
        specify_version: result.specify_version.clone(),
        wasm_pkg_config_written: result.wasm_pkg_config_written,
        context_generated: context_skip_reason.is_none(),
        context_skipped: context_skip_reason.is_some(),
        context_skip_reason,
    };
    output::emit(&mut std::io::stdout().lock(), format, &body, write_text)?;
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
    let outcome = agents::generate_for_init(&ctx)?;
    debug_assert!(
        outcome.changed,
        "init context generation is called only when AGENTS.md is absent"
    );
    debug_assert_eq!(outcome.disposition, "create");
    Ok(None)
}
