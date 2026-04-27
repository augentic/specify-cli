#![allow(clippy::needless_pass_by_value, clippy::items_after_statements)]

use std::path::PathBuf;

use serde::Serialize;
use serde_json::Value;
use specify::{Error, Phase, PipelineView, Schema, SchemaSource, ValidationResult};

use crate::cli::OutputFormat;
use crate::output::{CliResult, emit_error, emit_response};

use super::require_project;

pub fn run_schema_resolve(
    format: OutputFormat, schema_value: String, project_dir: PathBuf,
) -> CliResult {
    let resolved = match Schema::resolve(&schema_value, &project_dir) {
        Ok(r) => r,
        Err(err) => return emit_error(format, &err),
    };
    let (source, path) = match &resolved.source {
        SchemaSource::Local(p) => ("local", p.clone()),
        SchemaSource::Cached(p) => ("cached", p.clone()),
        _ => unreachable!(),
    };

    #[derive(Serialize)]
    #[serde(rename_all = "kebab-case")]
    struct SchemaResolveResponse {
        schema_value: String,
        resolved_path: String,
        source: &'static str,
    }
    match format {
        OutputFormat::Json => emit_response(SchemaResolveResponse {
            schema_value,
            resolved_path: path.display().to_string(),
            source,
        }),
        OutputFormat::Text => println!("{}", path.display()),
    }
    CliResult::Success
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct PipelineBriefJson {
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
struct SchemaPipelineResponse {
    phase: String,
    change: Option<String>,
    briefs: Vec<Value>,
}

pub fn run_schema_pipeline(
    format: OutputFormat, phase: Phase, change: Option<PathBuf>,
) -> CliResult {
    let (project_dir, config) = match require_project() {
        Ok(v) => v,
        Err(err) => return emit_error(format, &err),
    };
    let pipeline = match PipelineView::load(&config.schema, &project_dir) {
        Ok(view) => view,
        Err(err) => return emit_error(format, &err),
    };

    let order = match pipeline.topo_order(phase) {
        Ok(v) => v,
        Err(err) => return emit_error(format, &err),
    };
    let completion = change.as_deref().map(|change_dir| pipeline.completion_for(phase, change_dir));

    match format {
        OutputFormat::Json => {
            let briefs: Vec<Value> = order
                .iter()
                .map(|b| {
                    let present = completion.as_ref().and_then(|c| c.get(&b.frontmatter.id));
                    serde_json::to_value(PipelineBriefJson {
                        id: b.frontmatter.id.clone(),
                        description: b.frontmatter.description.clone(),
                        path: b.path.display().to_string(),
                        needs: b.frontmatter.needs.clone(),
                        generates: b.frontmatter.generates.clone(),
                        tracks: b.frontmatter.tracks.clone(),
                        present: present.copied().map_or(Value::Null, Value::from),
                    })
                    .expect("PipelineBriefJson serialises")
                })
                .collect();
            emit_response(SchemaPipelineResponse {
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
    CliResult::Success
}

pub fn run_schema_check(format: OutputFormat, schema_dir: PathBuf) -> CliResult {
    let schema_path = schema_dir.join("schema.yaml");
    let text = match std::fs::read_to_string(&schema_path) {
        Ok(t) => t,
        Err(err) => return emit_error(format, &Error::Io(err)),
    };
    let schema: Schema = match serde_saphyr::from_str(&text) {
        Ok(s) => s,
        Err(err) => return emit_error(format, &Error::Yaml(err)),
    };
    let results = schema.validate_structure();
    let passed = !results.iter().any(|r| matches!(r, ValidationResult::Fail { .. }));

    match format {
        OutputFormat::Json => {
            #[derive(Serialize)]
            #[serde(rename_all = "kebab-case")]
            struct SchemaCheckResponse {
                passed: bool,
                results: Vec<Value>,
            }
            let results_json: Vec<Value> = results.iter().map(validation_result_to_json).collect();
            emit_response(SchemaCheckResponse {
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
    if passed { CliResult::Success } else { CliResult::ValidationFailed }
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
#[serde(tag = "status")]
enum ValidationResultJson<'a> {
    #[serde(rename = "pass")]
    Pass { rule_id: &'a str, rule: &'a str },
    #[serde(rename = "fail")]
    Fail { rule_id: &'a str, rule: &'a str, detail: &'a str },
    #[serde(rename = "deferred")]
    Deferred { rule_id: &'a str, rule: &'a str, reason: &'a str },
}

fn validation_result_to_json(r: &ValidationResult) -> Value {
    let typed = match r {
        ValidationResult::Pass { rule_id, rule } => ValidationResultJson::Pass { rule_id, rule },
        ValidationResult::Fail {
            rule_id,
            rule,
            detail,
        } => ValidationResultJson::Fail {
            rule_id,
            rule,
            detail,
        },
        ValidationResult::Deferred {
            rule_id,
            rule,
            reason,
        } => ValidationResultJson::Deferred {
            rule_id,
            rule,
            reason,
        },
        _ => unreachable!(),
    };
    serde_json::to_value(typed).expect("ValidationResultJson serialises")
}
