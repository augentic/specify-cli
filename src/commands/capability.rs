#![allow(
    clippy::items_after_statements,
    clippy::needless_pass_by_value,
    reason = "Clap dispatch hands owned subcommand values to these command handlers."
)]

//! `specify capability {resolve, check, pipeline}` (RFC-13 Phase 1.2).
//!
//! These verbs replace the pre-RFC-13 `specify schema *` surface. The
//! command layer is intentionally where the post-RFC-13 manifest
//! filename (`capability.yaml`) is enforced: the lower-level
//! [`specify_capability::Capability`] resolver still tolerates the
//! legacy `schema.yaml` so internal callers like `init` keep working,
//! but the binary CLI refuses to load a directory that carries only
//! `schema.yaml` — it emits [`Error::LegacyCapabilityField`] instead
//! and points the operator at RFC-13 §Migration.

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;
use specify::{
    CAPABILITY_FILENAME, Capability, CapabilitySource, Error, ManifestProbe, Phase,
    ValidationResult,
};

use crate::cli::OutputFormat;
use crate::context::CommandContext;
use crate::output::{CliResult, emit_response};

pub fn resolve(
    format: OutputFormat, capability_value: String, project_dir: PathBuf,
) -> Result<CliResult, Error> {
    let (root_dir, source) = Capability::locate(&capability_value, &project_dir)?;
    enforce_capability_filename(&root_dir)?;
    let (source_label, path) = match &source {
        CapabilitySource::Local(p) => ("local", p.clone()),
        CapabilitySource::Cached(p) => ("cached", p.clone()),
        _ => ("unknown", PathBuf::new()),
    };

    #[derive(Serialize)]
    #[serde(rename_all = "kebab-case")]
    struct ResolveBody {
        capability_value: String,
        resolved_path: String,
        source: &'static str,
    }
    match format {
        OutputFormat::Json => emit_response(ResolveBody {
            capability_value,
            resolved_path: path.display().to_string(),
            source: source_label,
        })?,
        OutputFormat::Text => println!("{}", path.display()),
    }
    Ok(CliResult::Success)
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct BriefRow {
    id: String,
    description: String,
    path: String,
    needs: Vec<String>,
    generates: Option<String>,
    tracks: Option<String>,
    present: Value,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct PipelineBody {
    phase: String,
    slice: Option<String>,
    briefs: Vec<Value>,
}

pub fn pipeline(
    ctx: &CommandContext, phase: Phase, slice: Option<PathBuf>,
) -> Result<CliResult, Error> {
    let pipeline = ctx.load_pipeline()?;

    let order = pipeline.topo_order(phase)?;
    let completion = slice.as_deref().map(|slice_dir| pipeline.completion_for(phase, slice_dir));

    match ctx.format {
        OutputFormat::Json => {
            let briefs: Vec<Value> = order
                .iter()
                .map(|b| {
                    let present = completion.as_ref().and_then(|c| c.get(&b.frontmatter.id));
                    serde_json::to_value(BriefRow {
                        id: b.frontmatter.id.clone(),
                        description: b.frontmatter.description.clone(),
                        path: b.path.display().to_string(),
                        needs: b.frontmatter.needs.clone(),
                        generates: b.frontmatter.generates.clone(),
                        tracks: b.frontmatter.tracks.clone(),
                        present: present.copied().map_or(Value::Null, Value::from),
                    })
                    .expect("BriefRow serialises")
                })
                .collect();
            emit_response(PipelineBody {
                phase: phase.to_string(),
                slice: slice.as_ref().map(|p| p.display().to_string()),
                briefs,
            })?;
        }
        OutputFormat::Text => {
            println!("phase: {phase}");
            for b in &order {
                let present_label = completion
                    .as_ref()
                    .and_then(|c| c.get(&b.frontmatter.id))
                    .copied()
                    .map_or("", |p| if p { " [x]" } else { " [ ]" });
                println!("  {}{present_label}", b.frontmatter.id);
                if let Some(g) = &b.frontmatter.generates {
                    println!("    generates: {g}");
                }
                if !b.frontmatter.needs.is_empty() {
                    println!("    needs: {}", b.frontmatter.needs.join(", "));
                }
                if let Some(t) = &b.frontmatter.tracks {
                    println!("    tracks: {t}");
                }
            }
        }
    }
    Ok(CliResult::Success)
}

pub fn check(format: OutputFormat, capability_dir: PathBuf) -> Result<CliResult, Error> {
    let manifest_path = match Capability::probe_dir(&capability_dir) {
        ManifestProbe::Found(path) => path,
        ManifestProbe::Legacy(path) => {
            return Err(Error::LegacyCapabilityField { path });
        }
        ManifestProbe::Missing => {
            return Err(Error::SchemaResolution(format!(
                "no `{CAPABILITY_FILENAME}` at {}",
                capability_dir.display()
            )));
        }
    };
    let capability = load_capability(&manifest_path)?;
    let results = capability.validate_structure();
    let passed = !results.iter().any(|r| matches!(r, ValidationResult::Fail { .. }));

    match format {
        OutputFormat::Json => {
            #[derive(Serialize)]
            #[serde(rename_all = "kebab-case")]
            struct CheckBody {
                passed: bool,
                results: Vec<Value>,
            }
            let results_json: Vec<Value> = results.iter().map(validation_result_to_json).collect();
            emit_response(CheckBody {
                passed,
                results: results_json,
            })?;
        }
        OutputFormat::Text => {
            if passed {
                println!("Capability OK");
            } else {
                let fail_count =
                    results.iter().filter(|r| matches!(r, ValidationResult::Fail { .. })).count();
                println!("Capability invalid: {fail_count} errors");
                for r in &results {
                    if let ValidationResult::Fail { rule_id, detail, .. } = r {
                        println!("  [fail] {rule_id}: {detail}");
                    }
                }
            }
        }
    }
    Ok(if passed { CliResult::Success } else { CliResult::ValidationFailed })
}

/// Translate a directory probe into the post-RFC-13 invariant: the CLI
/// surface refuses to keep walking when a directory carries only the
/// pre-RFC-13 `schema.yaml`. Callers that succeed here are guaranteed
/// to find a `capability.yaml` on the next read.
fn enforce_capability_filename(dir: &Path) -> Result<(), Error> {
    match Capability::probe_dir(dir) {
        ManifestProbe::Found(_) => Ok(()),
        ManifestProbe::Legacy(path) => Err(Error::LegacyCapabilityField { path }),
        ManifestProbe::Missing => {
            Err(Error::SchemaResolution(format!("no `{CAPABILITY_FILENAME}` at {}", dir.display())))
        }
    }
}

fn load_capability(manifest_path: &Path) -> Result<Capability, Error> {
    let text = std::fs::read_to_string(manifest_path)?;
    let capability: Capability = serde_saphyr::from_str(&text)?;
    Ok(capability)
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
#[serde(tag = "status")]
enum CheckRow<'a> {
    #[serde(rename = "pass")]
    Pass { rule_id: &'a str, rule: &'a str },
    #[serde(rename = "fail")]
    Fail { rule_id: &'a str, rule: &'a str, detail: &'a str },
    #[serde(rename = "deferred")]
    Deferred { rule_id: &'a str, rule: &'a str, reason: &'a str },
}

fn validation_result_to_json(r: &ValidationResult) -> Value {
    let typed = match r {
        ValidationResult::Pass { rule_id, rule } => CheckRow::Pass { rule_id, rule },
        ValidationResult::Fail {
            rule_id,
            rule,
            detail,
        } => CheckRow::Fail {
            rule_id,
            rule,
            detail,
        },
        ValidationResult::Deferred {
            rule_id,
            rule,
            reason,
        } => CheckRow::Deferred {
            rule_id,
            rule,
            reason,
        },
        _ => {
            return serde_json::to_value(serde_json::json!({"status": "unknown"}))
                .expect("fallback JSON serialises");
        }
    };
    serde_json::to_value(typed).expect("CheckRow serialises")
}
