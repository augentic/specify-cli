#![allow(clippy::items_after_statements, clippy::needless_pass_by_value)]

use std::path::PathBuf;

use serde::Serialize;
use serde_json::Value;
use specify::{Error, Phase, Schema, SchemaSource, ValidationResult};

use crate::cli::OutputFormat;
use crate::context::CommandContext;
use crate::output::{CliResult, emit_response};

pub fn run_schema_resolve(
    format: OutputFormat, schema_value: String, project_dir: PathBuf,
) -> Result<CliResult, Error> {
    let resolved = Schema::resolve(&schema_value, &project_dir)?;
    let (source, path) = match &resolved.source {
        SchemaSource::Local(p) => ("local", p.clone()),
        SchemaSource::Cached(p) => ("cached", p.clone()),
        _ => ("unknown", PathBuf::new()),
    };

    #[derive(Serialize)]
    #[serde(rename_all = "kebab-case")]
    struct ResolveBody {
        schema_value: String,
        resolved_path: String,
        source: &'static str,
    }
    match format {
        OutputFormat::Json => emit_response(ResolveBody {
            schema_value,
            resolved_path: path.display().to_string(),
            source,
        }),
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
    change: Option<String>,
    briefs: Vec<Value>,
}

pub fn run_schema_pipeline(
    ctx: &CommandContext, phase: Phase, change: Option<PathBuf>,
) -> Result<CliResult, Error> {
    let pipeline = ctx.load_pipeline()?;

    let order = pipeline.topo_order(phase)?;
    let completion = change.as_deref().map(|change_dir| pipeline.completion_for(phase, change_dir));

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
                change: change.as_ref().map(|p| p.display().to_string()),
                briefs,
            });
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

pub fn run_schema_check(format: OutputFormat, schema_dir: PathBuf) -> Result<CliResult, Error> {
    let schema_path = schema_dir.join("schema.yaml");
    let text = std::fs::read_to_string(&schema_path)?;
    let schema: Schema = serde_saphyr::from_str(&text)?;
    let results = schema.validate_structure();
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
            });
        }
        OutputFormat::Text => {
            if passed {
                println!("Schema OK");
            } else {
                let fail_count =
                    results.iter().filter(|r| matches!(r, ValidationResult::Fail { .. })).count();
                println!("Schema invalid: {fail_count} errors");
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
