#![allow(
    clippy::needless_pass_by_value,
    reason = "Clap dispatch hands owned subcommand values to command handlers."
)]

use std::path::PathBuf;

use serde::Serialize;
use specify::{Error, InitOptions, InitResult, VersionMode, init};

use crate::cli::OutputFormat;
use crate::output::{CliResult, absolute_string, emit_response};

/// Dispatcher for `specify init`.
///
/// Enforces the RFC-13 Phase 1.3 mutual-exclusion invariant between the
/// `<capability>` positional and `--hub`:
///
/// - regular project init requires `<capability>`;
/// - hub init requires `--hub` and refuses a `<capability>` positional;
/// - missing both, or both at once, errors with
///   `init-requires-capability-or-hub`.
pub fn run_init(
    format: OutputFormat, capability: Option<String>, name: Option<String>, domain: Option<String>,
    hub: bool,
) -> Result<CliResult, Error> {
    let project_dir = PathBuf::from(".");

    let capability = match (hub, capability) {
        (false, Some(cap)) => Some(cap),
        (true, None) => None,
        // Both unset, or both set: the diagnostic is the same per
        // RFC-13 §1.3 — the operator must pick one.
        (false, None) | (true, Some(_)) => return Err(Error::InitRequiresCapabilityOrHub),
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
    emit_init_result(format, &result, hub)
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct InitBody {
    config_path: String,
    /// Resolved capability name (or `"hub"` for hub init). Kept under
    /// the legacy JSON key `schema-name` so existing wire consumers
    /// keep parsing while the vocabulary cut-over lands; renames live
    /// behind a structured-output bump out of scope for chunk 1.3.
    schema_name: String,
    cache_present: bool,
    directories_created: Vec<String>,
    scaffolded_rule_keys: Vec<String>,
    specify_version: String,
    /// `true` when this init scaffolded a registry-only platform hub
    /// (RFC-9 §1D). Always present so consumers can distinguish hub
    /// from regular initialisations without parsing the capability
    /// name.
    hub: bool,
}

fn emit_init_result(
    format: OutputFormat, result: &InitResult, hub: bool,
) -> Result<CliResult, Error> {
    match format {
        OutputFormat::Json => {
            emit_response(InitBody {
                config_path: absolute_string(&result.config_path),
                schema_name: result.capability_name.clone(),
                cache_present: result.cache_present,
                directories_created: result
                    .directories_created
                    .iter()
                    .map(|p| absolute_string(p))
                    .collect(),
                scaffolded_rule_keys: result.scaffolded_rule_keys.clone(),
                specify_version: result.specify_version.clone(),
                hub,
            })?;
        }
        OutputFormat::Text => {
            if hub {
                println!("Initialized .specify/ as a registry-only platform hub");
            } else {
                println!("Initialized .specify/");
            }
            println!("  capability: {}", result.capability_name);
            println!("  config: {}", absolute_string(&result.config_path));
            println!("  cache present: {}", result.cache_present);
            if !result.directories_created.is_empty() {
                println!(
                    "  directories created: {}",
                    result
                        .directories_created
                        .iter()
                        .map(|p| absolute_string(p))
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
            println!("  specify_version: {}", result.specify_version);
            // RFC-13 chunk 2.9 — init no longer pre-touches
            // platform-component artefacts; the hint points operators
            // at the verb that owns the next step.
            println!();
            if hub {
                println!(
                    "Next: run `specify registry add <id> <url>` to declare the projects \
                     this hub coordinates."
                );
            } else {
                println!(
                    "Next: run `specify change create <name>` to start a change, \
                     then `specify change plan create <name>` to plan it."
                );
            }
        }
    }
    Ok(CliResult::Success)
}
