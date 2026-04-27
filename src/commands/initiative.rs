use std::path::Path;

use serde::Serialize;
use serde_json::Value;
use specify::{Error, InitiativeBrief, Registry, is_valid_kebab_name};

use crate::cli::{BriefAction, InitiativeAction, OutputFormat, RegistryAction};
use crate::context::CommandContext;
use crate::output::{CliResult, absolute_string, emit_response};

pub fn run_initiative(
    ctx: &CommandContext,
    action: InitiativeAction,
) -> Result<CliResult, Error> {
    match action {
        InitiativeAction::Registry { action } => match action {
            RegistryAction::Show => run_initiative_registry_show(ctx),
            RegistryAction::Validate => run_initiative_registry_validate(ctx),
        },
        InitiativeAction::Brief { action } => match action {
            BriefAction::Init { name } => run_initiative_brief_init(ctx, name),
            BriefAction::Show => run_initiative_brief_show(ctx),
        },
    }
}

fn run_initiative_registry_show(ctx: &CommandContext) -> Result<CliResult, Error> {
    let registry_path = Registry::path(&ctx.project_dir);
    match Registry::load(&ctx.project_dir)? {
        None => {
            match ctx.format {
                OutputFormat::Json => {
                    #[derive(Serialize)]
                    #[serde(rename_all = "kebab-case")]
                    struct RegistryShowResponse {
                        registry: Value,
                        path: String,
                    }
                    emit_response(RegistryShowResponse {
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
                    struct RegistryShowFullResponse {
                        registry: Registry,
                        path: String,
                    }
                    emit_response(RegistryShowFullResponse {
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

fn run_initiative_registry_validate(ctx: &CommandContext) -> Result<CliResult, Error> {
    let registry_path = Registry::path(&ctx.project_dir);
    match Registry::load(&ctx.project_dir) {
        Ok(None) => {
            #[derive(Serialize)]
            #[serde(rename_all = "kebab-case")]
            struct RegistryValidateResponse {
                registry: Value,
                path: String,
                ok: bool,
            }
            match ctx.format {
                OutputFormat::Json => emit_response(RegistryValidateResponse {
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
            struct RegistryValidateFullResponse {
                registry: Registry,
                path: String,
                ok: bool,
            }
            match ctx.format {
                OutputFormat::Json => emit_response(RegistryValidateFullResponse {
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

fn run_initiative_brief_init(
    ctx: &CommandContext,
    name: String,
) -> Result<CliResult, Error> {
    if !is_valid_kebab_name(&name) {
        return Err(Error::Config(format!(
            "initiative.md: name `{name}` must be kebab-case \
             (lowercase ascii, digits, single hyphens; no leading/trailing/doubled hyphens)"
        )));
    }

    let brief_path = InitiativeBrief::path(&ctx.project_dir);
    if brief_path.exists() {
        match ctx.format {
            OutputFormat::Json => {
                #[derive(Serialize)]
                #[serde(rename_all = "kebab-case")]
                struct BriefInitErrorResponse {
                    action: &'static str,
                    ok: bool,
                    error: &'static str,
                    path: String,
                    exit_code: u8,
                }
                emit_response(BriefInitErrorResponse {
                    action: "init",
                    ok: false,
                    error: "already-exists",
                    path: brief_path.display().to_string(),
                    exit_code: CliResult::GenericFailure.code(),
                });
            }
            OutputFormat::Text => {
                eprintln!(
                    "initiative.md already exists at {}; refusing to overwrite",
                    brief_path.display()
                );
            }
        }
        return Ok(CliResult::GenericFailure);
    }

    if let Some(parent) = brief_path.parent() {
        std::fs::create_dir_all(parent).map_err(Error::Io)?;
    }
    let rendered = InitiativeBrief::template(&name);
    std::fs::write(&brief_path, &rendered).map_err(Error::Io)?;

    #[derive(Serialize)]
    #[serde(rename_all = "kebab-case")]
    struct BriefInitResponse {
        action: &'static str,
        ok: bool,
        name: String,
        path: String,
    }
    match ctx.format {
        OutputFormat::Json => emit_response(BriefInitResponse {
            action: "init",
            ok: true,
            name,
            path: absolute_string(&brief_path),
        }),
        OutputFormat::Text => {
            println!("Created .specify/initiative.md for {name}");
        }
    }
    Ok(CliResult::Success)
}

fn run_initiative_brief_show(ctx: &CommandContext) -> Result<CliResult, Error> {
    let brief_path = InitiativeBrief::path(&ctx.project_dir);
    match InitiativeBrief::load(&ctx.project_dir)? {
        None => {
            #[derive(Serialize)]
            #[serde(rename_all = "kebab-case")]
            struct BriefShowAbsentResponse {
                brief: Value,
                path: String,
            }
            match ctx.format {
                OutputFormat::Json => emit_response(BriefShowAbsentResponse {
                    brief: Value::Null,
                    path: brief_path.display().to_string(),
                }),
                OutputFormat::Text => {
                    println!("no initiative brief declared at .specify/initiative.md");
                }
            }
            Ok(CliResult::Success)
        }
        Some(brief) => {
            #[derive(Serialize)]
            #[serde(rename_all = "kebab-case")]
            struct BriefShowResponse {
                brief: BriefJson,
                path: String,
            }
            #[derive(Serialize)]
            #[serde(rename_all = "kebab-case")]
            struct BriefJson {
                frontmatter: specify::InitiativeFrontmatter,
                body: String,
            }
            match ctx.format {
                OutputFormat::Json => emit_response(BriefShowResponse {
                    brief: BriefJson {
                        frontmatter: brief.frontmatter.clone(),
                        body: brief.body,
                    },
                    path: brief_path.display().to_string(),
                }),
                OutputFormat::Text => print_initiative_brief_text(&brief, &brief_path),
            }
            Ok(CliResult::Success)
        }
    }
}

fn print_initiative_brief_text(brief: &InitiativeBrief, brief_path: &Path) {
    println!("initiative.md: {}", brief_path.display());
    println!("name: {}", brief.frontmatter.name);
    if brief.frontmatter.inputs.is_empty() {
        println!("inputs: (none)");
    } else {
        println!("inputs:");
        for input in &brief.frontmatter.inputs {
            let kind = match input.kind {
                specify::InputKind::LegacyCode => "legacy-code",
                specify::InputKind::Documentation => "documentation",
            };
            println!("  - path: {}", input.path);
            println!("    kind: {kind}");
        }
    }
    println!();
    print!("{}", brief.body);
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
