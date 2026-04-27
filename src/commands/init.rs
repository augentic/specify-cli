#![allow(clippy::needless_pass_by_value)]

use std::path::PathBuf;

use serde::Serialize;
use specify::{Error, InitOptions, InitResult, VersionMode, init};

use crate::cli::OutputFormat;
use crate::output::{CliResult, absolute_string, emit_response};

pub fn run_init(
    format: OutputFormat, schema: String, schema_dir: PathBuf, name: Option<String>,
    domain: Option<String>,
) -> Result<CliResult, Error> {
    let project_dir = PathBuf::from(".");

    let opts = InitOptions {
        project_dir: &project_dir,
        schema_value: &schema,
        schema_source_dir: &schema_dir,
        name: name.as_deref(),
        domain: domain.as_deref(),
        version_mode: VersionMode::WriteCurrent,
    };

    let result = init(opts)?;
    Ok(emit_init_result(format, &result))
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct InitResponse {
    config_path: String,
    schema_name: String,
    cache_present: bool,
    directories_created: Vec<String>,
    scaffolded_rule_keys: Vec<String>,
    specify_version: String,
}

fn emit_init_result(format: OutputFormat, result: &InitResult) -> CliResult {
    match format {
        OutputFormat::Json => {
            emit_response(InitResponse {
                config_path: absolute_string(&result.config_path),
                schema_name: result.schema_name.clone(),
                cache_present: result.cache_present,
                directories_created: result
                    .directories_created
                    .iter()
                    .map(|p| absolute_string(p))
                    .collect(),
                scaffolded_rule_keys: result.scaffolded_rule_keys.clone(),
                specify_version: result.specify_version.clone(),
            });
        }
        OutputFormat::Text => {
            println!("Initialized .specify/");
            println!("  schema: {}", result.schema_name);
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
        }
    }
    CliResult::Success
}
