#![allow(clippy::items_after_statements, clippy::option_if_let_else, clippy::unnecessary_wraps)]

use std::path::Path;

use serde::Serialize;
use serde_json::Value;
use specify::{Error, InitiativeBrief, is_valid_kebab_name};

use crate::cli::{InitiativeAction, OutputFormat};
use crate::context::CommandContext;
use crate::output::{CliResult, absolute_string, emit_response};

pub fn run_initiative(ctx: &CommandContext, action: InitiativeAction) -> Result<CliResult, Error> {
    match action {
        InitiativeAction::Init { name } => brief_init(ctx, name),
        InitiativeAction::Show => brief_show(ctx),
    }
}

fn brief_init(ctx: &CommandContext, name: String) -> Result<CliResult, Error> {
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
                struct BriefInitErr {
                    action: &'static str,
                    ok: bool,
                    error: &'static str,
                    path: String,
                    exit_code: u8,
                }
                emit_response(BriefInitErr {
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
    struct BriefInitOk {
        action: &'static str,
        ok: bool,
        name: String,
        path: String,
    }
    match ctx.format {
        OutputFormat::Json => emit_response(BriefInitOk {
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

fn brief_show(ctx: &CommandContext) -> Result<CliResult, Error> {
    let brief_path = InitiativeBrief::path(&ctx.project_dir);
    match InitiativeBrief::load(&ctx.project_dir)? {
        None => {
            #[derive(Serialize)]
            #[serde(rename_all = "kebab-case")]
            struct BriefAbsent {
                brief: Value,
                path: String,
            }
            match ctx.format {
                OutputFormat::Json => emit_response(BriefAbsent {
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
            struct BriefBody {
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
                OutputFormat::Json => emit_response(BriefBody {
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
