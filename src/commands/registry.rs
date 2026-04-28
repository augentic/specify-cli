#![allow(
    clippy::items_after_statements,
    clippy::needless_pass_by_value,
    clippy::option_if_let_else,
    clippy::unnecessary_wraps
)]

use std::path::Path;

use serde::Serialize;
use serde_json::Value;
use specify::{Error, Registry};

use crate::cli::{OutputFormat, RegistryAction};
use crate::context::CommandContext;
use crate::output::{CliResult, emit_response};

pub fn run_registry(ctx: &CommandContext, action: RegistryAction) -> Result<CliResult, Error> {
    match action {
        RegistryAction::Show => show_registry(ctx),
        RegistryAction::Validate => validate_registry(ctx),
    }
}

fn show_registry(ctx: &CommandContext) -> Result<CliResult, Error> {
    let registry_path = Registry::path(&ctx.project_dir);
    match Registry::load(&ctx.project_dir)? {
        None => {
            match ctx.format {
                OutputFormat::Json => {
                    #[derive(Serialize)]
                    #[serde(rename_all = "kebab-case")]
                    struct RegistryBody {
                        registry: Value,
                        path: String,
                    }
                    emit_response(RegistryBody {
                        registry: Value::Null,
                        path: registry_path.display().to_string(),
                    });
                }
                OutputFormat::Text => {
                    println!("no registry declared at .specify/registry.yaml");
                }
            }
            Ok(CliResult::Success)
        }
        Some(registry) => {
            match ctx.format {
                OutputFormat::Json => {
                    #[derive(Serialize)]
                    #[serde(rename_all = "kebab-case")]
                    struct RegistryFullBody {
                        registry: Registry,
                        path: String,
                    }
                    emit_response(RegistryFullBody {
                        registry,
                        path: registry_path.display().to_string(),
                    });
                }
                OutputFormat::Text => {
                    print_registry_text(&registry, &registry_path);
                }
            }
            Ok(CliResult::Success)
        }
    }
}

fn validate_registry(ctx: &CommandContext) -> Result<CliResult, Error> {
    let registry_path = Registry::path(&ctx.project_dir);
    match Registry::load(&ctx.project_dir) {
        Ok(None) => {
            #[derive(Serialize)]
            #[serde(rename_all = "kebab-case")]
            struct ValidateEmpty {
                registry: Value,
                path: String,
                ok: bool,
            }
            match ctx.format {
                OutputFormat::Json => emit_response(ValidateEmpty {
                    registry: Value::Null,
                    path: registry_path.display().to_string(),
                    ok: true,
                }),
                OutputFormat::Text => {
                    println!("no registry declared at .specify/registry.yaml");
                }
            }
            Ok(CliResult::Success)
        }
        Ok(Some(registry)) => {
            let count = registry.projects.len();
            #[derive(Serialize)]
            #[serde(rename_all = "kebab-case")]
            struct ValidateBody {
                registry: Registry,
                path: String,
                ok: bool,
            }
            match ctx.format {
                OutputFormat::Json => emit_response(ValidateBody {
                    registry,
                    path: registry_path.display().to_string(),
                    ok: true,
                }),
                OutputFormat::Text => {
                    println!("registry.yaml is well-formed ({count} project(s))");
                }
            }
            Ok(CliResult::Success)
        }
        Err(err) => {
            #[derive(Serialize)]
            #[serde(rename_all = "kebab-case")]
            struct RegistryValidateErrorResponse {
                path: String,
                ok: bool,
                error: String,
                kind: &'static str,
                exit_code: u8,
            }
            match ctx.format {
                OutputFormat::Json => emit_response(RegistryValidateErrorResponse {
                    path: registry_path.display().to_string(),
                    ok: false,
                    error: err.to_string(),
                    kind: "config",
                    exit_code: CliResult::ValidationFailed.code(),
                }),
                OutputFormat::Text => eprintln!("error: {err}"),
            }
            Ok(CliResult::ValidationFailed)
        }
    }
}

fn print_registry_text(registry: &Registry, registry_path: &Path) {
    println!("registry.yaml: {}", registry_path.display());
    println!("version: {}", registry.version);
    if registry.projects.is_empty() {
        println!("projects: (none)");
        return;
    }
    println!("projects:");
    for project in &registry.projects {
        println!("  - name: {}", project.name);
        println!("    url: {}", project.url);
        println!("    schema: {}", project.schema);
    }
}
