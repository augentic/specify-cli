#![allow(clippy::items_after_statements, clippy::needless_pass_by_value)]

use serde::Serialize;
use serde_json::Value;
use specify::{
    Error, Plan, PlanStatus, PlanValidationLevel, PlanValidationResult, ProjectConfig, Registry,
};

use super::{
    PlanRef, absolute_string, emit_structural_error, file_path, load_for_write, plan_ref,
    print_validation_line, require_file, validation_to_json,
};
use crate::cli::OutputFormat;
use crate::context::CommandContext;
use crate::output::{CliResult, emit_response};

pub fn run_plan_init(
    ctx: &CommandContext, name: String, sources: Vec<(String, String)>,
) -> Result<CliResult, Error> {
    let plan_path = file_path(&ctx.project_dir);
    if plan_path.exists() {
        return Err(Error::Config(format!(
            "plan already exists at {}; run `specify plan archive` first",
            plan_path.display()
        )));
    }

    let mut source_map: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();
    for (k, v) in sources {
        if source_map.contains_key(&k) {
            return Err(Error::Config(format!("duplicate key `{k}` in --source arguments")));
        }
        source_map.insert(k, v);
    }

    let plan = Plan::init(&name, source_map)?;
    plan.save(&plan_path)?;

    #[derive(Serialize)]
    #[serde(rename_all = "kebab-case")]
    struct InitBody {
        plan: PlanRef,
    }

    match ctx.format {
        OutputFormat::Json => emit_response(InitBody {
            plan: PlanRef {
                name,
                path: absolute_string(&plan_path),
            },
        }),
        OutputFormat::Text => {
            println!("Initialised plan '{name}' at {}.", plan_path.display());
        }
    }
    Ok(CliResult::Success)
}

pub fn run_plan_validate(ctx: &CommandContext) -> Result<CliResult, Error> {
    let plan_path = require_file(&ctx.project_dir)?;
    let plan = Plan::load(&plan_path)?;
    let changes_dir = ProjectConfig::changes_dir(&ctx.project_dir);

    let (registry, registry_err) = match Registry::load(&ctx.project_dir) {
        Ok(reg) => (reg, None),
        Err(err) => (None, Some(err)),
    };
    let mut results = plan.validate(Some(&changes_dir), registry.as_ref());
    if let Some(err) = registry_err {
        results.push(PlanValidationResult {
            level: PlanValidationLevel::Error,
            code: "registry-shape",
            message: err.to_string(),
            entry: None,
        });
    }

    if let Some(ref reg) = registry {
        let workspace_base = ProjectConfig::specify_dir(&ctx.project_dir).join("workspace");
        for rp in &reg.projects {
            let slot_project_yaml =
                workspace_base.join(&rp.name).join(".specify").join("project.yaml");
            if slot_project_yaml.exists()
                && let Ok(content) = std::fs::read_to_string(&slot_project_yaml)
                && let Ok(config) = serde_saphyr::from_str::<serde_json::Value>(&content)
                && let Some(schema_val) = config.get("schema").and_then(|v| v.as_str())
                && schema_val != rp.schema
            {
                results.push(PlanValidationResult {
                    level: PlanValidationLevel::Warning,
                    code: "schema-mismatch-workspace",
                    message: format!(
                        "workspace clone '{}' has schema '{}' but registry declares '{}'; \
                         the clone's project.yaml is authoritative at execution time",
                        rp.name, schema_val, rp.schema
                    ),
                    entry: None,
                });
            }
        }
    }

    let has_errors = results.iter().any(|r| matches!(r.level, PlanValidationLevel::Error));

    match ctx.format {
        OutputFormat::Json => {
            #[derive(Serialize)]
            #[serde(rename_all = "kebab-case")]
            struct PlanValidateResponse {
                plan: PlanRef,
                results: Vec<Value>,
                passed: bool,
            }
            let items: Vec<Value> = results.iter().map(validation_to_json).collect();
            emit_response(PlanValidateResponse {
                plan: PlanRef {
                    name: plan.name,
                    path: plan_path.display().to_string(),
                },
                results: items,
                passed: !has_errors,
            });
        }
        OutputFormat::Text => {
            for r in &results {
                print_validation_line(r);
            }
            if results.is_empty() {
                println!("Plan OK");
            }
        }
    }

    Ok(if has_errors { CliResult::ValidationFailed } else { CliResult::Success })
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct NextBody {
    next: Option<String>,
    reason: Option<String>,
    active: Option<String>,
    project: Option<String>,
    schema: Option<String>,
    description: Option<String>,
    sources: Option<Vec<String>>,
}

pub fn run_plan_next(ctx: &CommandContext) -> Result<CliResult, Error> {
    let plan_path = require_file(&ctx.project_dir)?;
    let plan = Plan::load(&plan_path)?;
    let changes_dir = ProjectConfig::changes_dir(&ctx.project_dir);

    let results = plan.validate(Some(&changes_dir), None);
    if results.iter().any(|r| matches!(r.level, PlanValidationLevel::Error)) {
        return Ok(emit_structural_error(ctx.format));
    }

    if let Some(active) = plan.changes.iter().find(|c| c.status == PlanStatus::InProgress) {
        match ctx.format {
            OutputFormat::Json => emit_response(NextBody {
                next: None,
                reason: Some("in-progress".to_string()),
                active: Some(active.name.clone()),
                project: None,
                schema: None,
                description: None,
                sources: None,
            }),
            OutputFormat::Text => println!("Active change in progress: {}", active.name),
        }
        return Ok(CliResult::Success);
    }

    if let Some(entry) = plan.next_eligible() {
        match ctx.format {
            OutputFormat::Json => emit_response(NextBody {
                next: Some(entry.name.clone()),
                reason: None,
                active: None,
                project: entry.project.clone(),
                schema: entry.schema.clone(),
                description: entry.description.clone(),
                sources: Some(entry.sources.clone()),
            }),
            OutputFormat::Text => println!("{}", entry.name),
        }
    } else {
        let all_terminal =
            plan.changes.iter().all(|c| matches!(c.status, PlanStatus::Done | PlanStatus::Skipped));
        let (reason, text_msg) = if all_terminal {
            ("all-done", "All changes done.")
        } else {
            (
                "stuck",
                "No eligible changes \u{2014} remaining entries are blocked, failed, or waiting on unmet dependencies.",
            )
        };
        match ctx.format {
            OutputFormat::Json => emit_response(NextBody {
                next: None,
                reason: Some(reason.to_string()),
                active: None,
                project: None,
                schema: None,
                description: None,
                sources: None,
            }),
            OutputFormat::Text => println!("{text_msg}"),
        }
    }
    Ok(CliResult::Success)
}

pub fn run_plan_transition(
    ctx: &CommandContext, name: String, target: PlanStatus, reason: Option<String>,
) -> Result<CliResult, Error> {
    let (plan_path, mut plan) = load_for_write(ctx)?;

    let old_status = plan
        .changes
        .iter()
        .find(|c| c.name == name)
        .ok_or_else(|| Error::Config(format!("no change named '{name}' in plan")))?
        .status;

    plan.transition(&name, target, reason.as_deref())?;
    plan.save(&plan_path)?;

    let entry = plan.changes.iter().find(|c| c.name == name).expect("transitioned entry present");

    #[derive(Serialize)]
    #[serde(rename_all = "kebab-case")]
    struct TransitionBody {
        plan: PlanRef,
        entry: TransitionRow,
    }
    #[derive(Serialize)]
    #[serde(rename_all = "kebab-case")]
    struct TransitionRow {
        name: String,
        status: String,
        status_reason: Option<String>,
    }
    match ctx.format {
        OutputFormat::Json => emit_response(TransitionBody {
            plan: plan_ref(&plan, &plan_path),
            entry: TransitionRow {
                name: entry.name.clone(),
                status: entry.status.to_string(),
                status_reason: entry.status_reason.clone(),
            },
        }),
        OutputFormat::Text => {
            println!("Transitioned '{name}': {} \u{2192} {}.", old_status, entry.status);
        }
    }
    Ok(CliResult::Success)
}

pub fn run_plan_archive(ctx: &CommandContext, force: bool) -> Result<CliResult, Error> {
    let plan_path = ctx.project_dir.join(".specify/plan.yaml");
    if !plan_path.exists() {
        return Err(Error::ArtifactNotFound {
            kind: "plan.yaml",
            path: plan_path,
        });
    }
    let archive_dir = ProjectConfig::archive_dir(&ctx.project_dir).join("plans");

    let plan_name = Plan::load(&plan_path)?.name;

    match Plan::archive(&plan_path, &archive_dir, force) {
        Ok((archived, archived_plans_dir)) => {
            match ctx.format {
                OutputFormat::Json => {
                    #[derive(Serialize)]
                    #[serde(rename_all = "kebab-case")]
                    struct ArchiveBody {
                        archived: String,
                        archived_plans_dir: Option<String>,
                        plan: ArchiveId,
                    }
                    #[derive(Serialize)]
                    #[serde(rename_all = "kebab-case")]
                    struct ArchiveId {
                        name: String,
                    }
                    emit_response(ArchiveBody {
                        archived: absolute_string(&archived),
                        archived_plans_dir: archived_plans_dir.as_deref().map(absolute_string),
                        plan: ArchiveId { name: plan_name },
                    });
                }
                OutputFormat::Text => match archived_plans_dir {
                    Some(dir) => println!(
                        "Archived plan to {}. Working directory moved to {}.",
                        archived.display(),
                        dir.display()
                    ),
                    None => println!("Archived plan to {}.", archived.display()),
                },
            }
            Ok(CliResult::Success)
        }
        Err(Error::PlanHasOutstandingWork { entries }) => {
            match ctx.format {
                OutputFormat::Json => {
                    #[derive(Serialize)]
                    #[serde(rename_all = "kebab-case")]
                    struct OutstandingWork {
                        error: &'static str,
                        entries: Vec<String>,
                        exit_code: u8,
                    }
                    emit_response(OutstandingWork {
                        error: "plan-has-outstanding-work",
                        entries,
                        exit_code: CliResult::GenericFailure.code(),
                    });
                }
                OutputFormat::Text => {
                    eprintln!(
                        "Refusing to archive \u{2014} outstanding non-terminal entries: {}. Re-run with --force to archive anyway.",
                        entries.join(", ")
                    );
                }
            }
            Ok(CliResult::GenericFailure)
        }
        Err(err) => Err(err),
    }
}
