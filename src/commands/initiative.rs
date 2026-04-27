#![allow(clippy::items_after_statements)]

use std::path::Path;

use serde::Serialize;
use serde_json::Value;
use specify::{Error, InitiativeBrief, Registry, is_valid_kebab_name};

use crate::cli::{BriefAction, InitiativeAction, OutputFormat, RegistryAction};
use crate::output::{CliResult, absolute_string, emit_error, emit_response};

use super::require_project;

pub fn run_initiative(format: OutputFormat, action: InitiativeAction) -> CliResult {
    match action {
        InitiativeAction::Registry { action } => match action {
            RegistryAction::Show => run_initiative_registry_show(format),
            RegistryAction::Validate => run_initiative_registry_validate(format),
        },
        InitiativeAction::Brief { action } => match action {
            BriefAction::Init { name } => run_initiative_brief_init(format, name),
            BriefAction::Show => run_initiative_brief_show(format),
        },
    }
}

/// `specify initiative registry show` — print the parsed registry in
/// text or JSON. `Err` on malformed YAML (fail loud; the user asked to
/// show something unparseable). `Ok(None)` is not an error.
fn run_initiative_registry_show(format: OutputFormat) -> CliResult {
    let (project_dir, _config) = match require_project() {
        Ok(v) => v,
        Err(err) => return emit_error(format, &err),
    };
    let registry_path = Registry::path(&project_dir);
    match Registry::load(&project_dir) {
        Ok(None) => match format {
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
                CliResult::Success
            }
            OutputFormat::Text => {
                println!("no registry declared at .specify/registry.yaml");
                CliResult::Success
            }
        },
        Ok(Some(registry)) => {
            match format {
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
            CliResult::Success
        }
        Err(err) => emit_error(format, &err),
    }
}

/// `specify initiative registry validate` — dedicated verb for the same
/// shape check `plan validate` runs via its C12 hook. Exits
/// `CliResult::ValidationFailed` (2) on malformed input; 0 otherwise,
/// including when `.specify/registry.yaml` is absent.
fn run_initiative_registry_validate(format: OutputFormat) -> CliResult {
    let (project_dir, _config) = match require_project() {
        Ok(v) => v,
        Err(err) => return emit_error(format, &err),
    };
    let registry_path = Registry::path(&project_dir);
    match Registry::load(&project_dir) {
        Ok(None) => {
            #[derive(Serialize)]
            #[serde(rename_all = "kebab-case")]
            struct RegistryValidateResponse {
                registry: Value,
                path: String,
                ok: bool,
            }
            match format {
                OutputFormat::Json => emit_response(RegistryValidateResponse {
                    registry: Value::Null,
                    path: registry_path.display().to_string(),
                    ok: true,
                }),
                OutputFormat::Text => {
                    println!("no registry declared at .specify/registry.yaml");
                }
            }
            CliResult::Success
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
            match format {
                OutputFormat::Json => emit_response(RegistryValidateFullResponse {
                    registry,
                    path: registry_path.display().to_string(),
                    ok: true,
                }),
                OutputFormat::Text => {
                    println!("registry.yaml is well-formed ({count} project(s))");
                }
            }
            CliResult::Success
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
            match format {
                OutputFormat::Json => emit_response(RegistryValidateErrorResponse {
                    path: registry_path.display().to_string(),
                    ok: false,
                    error: err.to_string(),
                    kind: "config",
                    exit_code: CliResult::ValidationFailed.code(),
                }),
                OutputFormat::Text => eprintln!("error: {err}"),
            }
            CliResult::ValidationFailed
        }
    }
}

/// `specify initiative brief init <name>` — scaffold
/// `.specify/initiative.md` from the canonical template. Refuses to
/// overwrite an existing file; rejects non-kebab-case names before
/// touching disk.
fn run_initiative_brief_init(format: OutputFormat, name: String) -> CliResult {
    let (project_dir, _config) = match require_project() {
        Ok(v) => v,
        Err(err) => return emit_error(format, &err),
    };

    if !is_valid_kebab_name(&name) {
        let err = Error::Config(format!(
            "initiative.md: name `{name}` must be kebab-case \
             (lowercase ascii, digits, single hyphens; no leading/trailing/doubled hyphens)"
        ));
        return emit_error(format, &err);
    }

    let brief_path = InitiativeBrief::path(&project_dir);
    if brief_path.exists() {
        match format {
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
        return CliResult::GenericFailure;
    }

    if let Some(parent) = brief_path.parent()
        && let Err(err) = std::fs::create_dir_all(parent)
    {
        return emit_error(format, &Error::Io(err));
    }
    let rendered = InitiativeBrief::template(&name);
    if let Err(err) = std::fs::write(&brief_path, &rendered) {
        return emit_error(format, &Error::Io(err));
    }

    #[derive(Serialize)]
    #[serde(rename_all = "kebab-case")]
    struct BriefInitResponse {
        action: &'static str,
        ok: bool,
        name: String,
        path: String,
    }
    match format {
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
    CliResult::Success
}

/// `specify initiative brief show` — print the parsed brief in text or
/// JSON. Absent file exits 0 with a "no initiative brief declared"
/// message; malformed files fail loud — the operator asked to show
/// something unparseable.
fn run_initiative_brief_show(format: OutputFormat) -> CliResult {
    let (project_dir, _config) = match require_project() {
        Ok(v) => v,
        Err(err) => return emit_error(format, &err),
    };
    let brief_path = InitiativeBrief::path(&project_dir);
    match InitiativeBrief::load(&project_dir) {
        Ok(None) => {
            #[derive(Serialize)]
            #[serde(rename_all = "kebab-case")]
            struct BriefShowAbsentResponse {
                brief: Value,
                path: String,
            }
            match format {
                OutputFormat::Json => emit_response(BriefShowAbsentResponse {
                    brief: Value::Null,
                    path: brief_path.display().to_string(),
                }),
                OutputFormat::Text => {
                    println!("no initiative brief declared at .specify/initiative.md");
                }
            }
            CliResult::Success
        }
        Ok(Some(brief)) => {
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
            match format {
                OutputFormat::Json => emit_response(BriefShowResponse {
                    brief: BriefJson {
                        frontmatter: brief.frontmatter.clone(),
                        body: brief.body,
                    },
                    path: brief_path.display().to_string(),
                }),
                OutputFormat::Text => print_initiative_brief_text(&brief, &brief_path),
            }
            CliResult::Success
        }
        Err(err) => emit_error(format, &err),
    }
}

/// Plain text dump for `specify initiative brief show`. Not
/// golden-tested — structured consumers use `--format json`.
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

/// Plain, two-space-indented registry summary for `--format text`. Not
/// golden-tested — structured consumers use `--format json`.
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
