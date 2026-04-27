#![allow(clippy::items_after_statements)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;
use specify::{
    ChangeMetadata, Error, Plan, PlanChange, PlanChangePatch, PlanLockAcquired, PlanLockReleased,
    PlanLockStamp, PlanLockState, PlanStatus, PlanValidationLevel, PlanValidationResult,
    ProjectConfig, Registry,
};

use crate::cli::{LockAction, OutputFormat, PlanAction};
use crate::context::CommandContext;
use crate::output::{CliResult, absolute_string, emit_response};

pub fn run_plan(ctx: &CommandContext, action: PlanAction) -> Result<CliResult, Error> {
    match action {
        PlanAction::Init { name, sources } => run_initiative_init(ctx, name, sources),
        PlanAction::Validate => run_initiative_validate(ctx),
        PlanAction::Next => run_initiative_next(ctx),
        PlanAction::Status => run_initiative_status(ctx),
        PlanAction::Create {
            name,
            depends_on,
            sources,
            description,
            project,
            schema,
            context,
        } => run_initiative_create(ctx, name, depends_on, sources, description, project, schema, context),
        PlanAction::Amend {
            name,
            depends_on,
            sources,
            description,
            project,
            schema,
            context,
        } => run_initiative_amend(ctx, name, depends_on, sources, description, project, schema, context),
        PlanAction::Transition { name, target, reason } => {
            run_initiative_transition(ctx, name, target, reason)
        }
        PlanAction::Archive { force } => run_initiative_archive(ctx, force),
        PlanAction::Lock { action } => match action {
            LockAction::Acquire { pid } => run_initiative_lock_acquire(ctx, pid),
            LockAction::Release { pid } => run_initiative_lock_release(ctx, pid),
            LockAction::Status => run_initiative_lock_status(ctx),
        },
    }
}

/// `<project_dir>/.specify/plan.yaml`.
pub fn plan_file_path(project_dir: &Path) -> PathBuf {
    ProjectConfig::specify_dir(project_dir).join("plan.yaml")
}

/// Ensure the plan file exists before we try to load it. Error text is
/// the stable "plan file not found: .specify/plan.yaml" string that
/// skill authors match on.
pub fn require_plan_file(project_dir: &Path) -> Result<PathBuf, Error> {
    let path = plan_file_path(project_dir);
    if !path.exists() {
        return Err(Error::ArtifactNotFound {
            kind: "plan.yaml",
            path,
        });
    }
    Ok(path)
}

const fn plan_validation_level_label(level: &PlanValidationLevel) -> &'static str {
    match level {
        PlanValidationLevel::Error => "error",
        PlanValidationLevel::Warning => "warning",
    }
}

fn run_initiative_init(
    ctx: &CommandContext, name: String, sources: Vec<(String, String)>,
) -> Result<CliResult, Error> {
    let plan_path = plan_file_path(&ctx.project_dir);
    if plan_path.exists() {
        return Err(Error::Config(format!(
            "plan already exists at {}; run `specify plan archive` first",
            plan_path.display()
        )));
    }

    // Fold the CLI vector into a BTreeMap, rejecting duplicate keys
    // before they silently clobber earlier values.
    let mut source_map: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();
    for (k, v) in sources {
        if source_map.contains_key(&k) {
            return Err(Error::Config(format!(
                "duplicate key `{k}` in --source arguments"
            )));
        }
        source_map.insert(k, v);
    }

    let plan = Plan::init(&name, source_map)?;
    plan.save(&plan_path)?;

    #[derive(Serialize)]
    #[serde(rename_all = "kebab-case")]
    struct PlanInitResponse {
        plan: PlanRef,
    }

    match ctx.format {
        OutputFormat::Json => emit_response(PlanInitResponse {
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

fn run_initiative_validate(ctx: &CommandContext) -> Result<CliResult, Error> {
    let plan_path = require_plan_file(&ctx.project_dir)?;
    let plan = Plan::load(&plan_path)?;
    let changes_dir = ProjectConfig::changes_dir(&ctx.project_dir);

    let registry = Registry::load(&ctx.project_dir).ok().flatten();
    let mut results = plan.validate(Some(&changes_dir), registry.as_ref());
    if let Err(err) = Registry::load(&ctx.project_dir) {
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
            let items: Vec<Value> = results.iter().map(plan_validation_to_json).collect();
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
                print_plan_validation_line(r);
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
struct PlanValidationJson<'a> {
    level: &'a str,
    code: &'a str,
    entry: &'a Option<String>,
    message: &'a str,
}

fn plan_validation_to_json(r: &PlanValidationResult) -> Value {
    serde_json::to_value(PlanValidationJson {
        level: plan_validation_level_label(&r.level),
        code: r.code,
        entry: &r.entry,
        message: &r.message,
    })
    .expect("PlanValidationJson serialises")
}

/// Roughly-columnar single line per finding. Not golden-tested — skills
/// that need structure consume `--format json`.
fn print_plan_validation_line(r: &PlanValidationResult) {
    let level = match r.level {
        PlanValidationLevel::Error => "ERROR  ",
        PlanValidationLevel::Warning => "WARNING",
    };
    let entry_col = r.entry.as_ref().map_or_else(String::new, |e| format!("[{e}]"));
    println!("{level} {:<32} {:<24} {}", r.code, entry_col, r.message);
}

/// Emit the stable "go run `specify plan validate`" pointer when
/// `plan next` or `plan status` is asked to operate on a
/// structurally broken plan.
fn emit_plan_structural_error(format: OutputFormat) -> CliResult {
    let msg = "plan has structural errors; run 'specify plan validate' for detail";
    match format {
        OutputFormat::Json => emit_response(crate::output::ErrorResponse {
            error: "validation".to_string(),
            message: msg.to_string(),
            exit_code: CliResult::ValidationFailed.code(),
        }),
        OutputFormat::Text => eprintln!("error: {msg}"),
    }
    CliResult::ValidationFailed
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct PlanNextResponse {
    next: Option<String>,
    reason: Option<String>,
    active: Option<String>,
    project: Option<String>,
    schema: Option<String>,
    description: Option<String>,
    sources: Option<Vec<String>>,
}

fn run_initiative_next(ctx: &CommandContext) -> Result<CliResult, Error> {
    let plan_path = require_plan_file(&ctx.project_dir)?;
    let plan = Plan::load(&plan_path)?;
    let changes_dir = ProjectConfig::changes_dir(&ctx.project_dir);

    // `plan next` deliberately skips the filesystem-aware
    // `scope-path-missing` sweep (project_dir = None): a scope path
    // may be transiently absent during a rename or partial checkout
    // and should not block driver progression. `plan validate`
    // is the place to surface those.
    let results = plan.validate(Some(&changes_dir), None);
    if results.iter().any(|r| matches!(r.level, PlanValidationLevel::Error)) {
        return Ok(emit_plan_structural_error(ctx.format));
    }

    if let Some(active) = plan.changes.iter().find(|c| c.status == PlanStatus::InProgress) {
        match ctx.format {
            OutputFormat::Json => emit_response(PlanNextResponse {
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
            OutputFormat::Json => emit_response(PlanNextResponse {
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
        // Classify the "None" branch: fully-finished initiative vs
        // still-has-work-but-blocked. An empty plan falls out of the
        // `all` check as "all-done" (vacuously true).
        let all_terminal =
            plan.changes.iter().all(|c| matches!(c.status, PlanStatus::Done | PlanStatus::Skipped));
        let (reason, text_msg) = if all_terminal {
            ("all-done", "All changes done.")
        } else {
            (
                "stuck",
                "No eligible changes — remaining entries are blocked, failed, or waiting on unmet dependencies.",
            )
        };
        match ctx.format {
            OutputFormat::Json => emit_response(PlanNextResponse {
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

#[allow(clippy::too_many_lines)]
fn run_initiative_status(ctx: &CommandContext) -> Result<CliResult, Error> {
    let plan_path = require_plan_file(&ctx.project_dir)?;
    let plan = Plan::load(&plan_path)?;
    let changes_dir = ProjectConfig::changes_dir(&ctx.project_dir);

    // `plan status` stays permissive by design — see the
    // `dependency-cycle` fallback below. Running the
    // `scope-path-missing` sweep here would add a second class of
    // error that has to be tolerated; defer filesystem-aware
    // diagnostics to `plan validate`.
    let results = plan.validate(Some(&changes_dir), None);
    // Cycle is recoverable (we fall back to list order); any *other*
    // structural error (duplicate-name / unknown-depends-on /
    // unknown-source / multiple-in-progress) is fatal.
    let has_other_structural_errors = results
        .iter()
        .any(|r| matches!(r.level, PlanValidationLevel::Error) && r.code != "dependency-cycle");
    if has_other_structural_errors {
        return Ok(emit_plan_structural_error(ctx.format));
    }

    let (ordered, order_label) = if let Ok(v) = plan.topological_order() {
        (v, "topological")
    } else {
        match ctx.format {
            OutputFormat::Json => {
                eprintln!(
                    "warning: dependency cycle detected — falling back to list order. Run 'specify plan validate' for detail."
                );
            }
            OutputFormat::Text => {
                println!(
                    "⚠ dependency cycle detected — falling back to list order. Run 'specify plan validate' for detail."
                );
            }
        }
        (plan.changes.iter().collect::<Vec<_>>(), "list")
    };

    let mut counts: BTreeMap<PlanStatus, usize> = PlanStatus::ALL.iter().map(|&s| (s, 0)).collect();
    for entry in &plan.changes {
        *counts.get_mut(&entry.status).expect("ALL covers status") += 1;
    }
    let total: usize = counts.values().sum();

    let active = plan.changes.iter().find(|c| c.status == PlanStatus::InProgress);
    let active_lifecycle = active.and_then(|a| load_lifecycle_label(&changes_dir.join(&a.name)));

    let blocked: Vec<&PlanChange> =
        plan.changes.iter().filter(|c| c.status == PlanStatus::Blocked).collect();
    let failed: Vec<&PlanChange> =
        plan.changes.iter().filter(|c| c.status == PlanStatus::Failed).collect();

    let next_eligible = plan.next_eligible();

    match ctx.format {
        OutputFormat::Json => {
            #[derive(Serialize)]
            #[serde(rename_all = "kebab-case")]
            struct PlanStatusResponse {
                plan: PlanRef,
                counts: PlanCounts,
                order: &'static str,
                entries: Vec<Value>,
                in_progress: Option<PlanActiveJson>,
                blocked: Vec<PlanNameReason>,
                failed: Vec<PlanNameReason>,
                next_eligible: Option<String>,
            }
            #[derive(Serialize)]
            #[serde(rename_all = "kebab-case")]
            struct PlanCounts {
                done: usize,
                in_progress: usize,
                pending: usize,
                blocked: usize,
                failed: usize,
                skipped: usize,
                total: usize,
            }
            #[derive(Serialize)]
            #[serde(rename_all = "kebab-case")]
            struct PlanActiveJson {
                name: String,
                lifecycle: Option<String>,
            }
            #[derive(Serialize)]
            #[serde(rename_all = "kebab-case")]
            struct PlanNameReason {
                name: String,
                reason: Option<String>,
            }

            let entries: Vec<Value> = ordered
                .iter()
                .map(|entry| {
                    let lifecycle = if entry.status == PlanStatus::InProgress {
                        active_lifecycle.clone()
                    } else {
                        None
                    };
                    plan_entry_to_json(entry, lifecycle)
                })
                .collect();

            let blocked_json: Vec<PlanNameReason> = blocked
                .iter()
                .map(|c| PlanNameReason {
                    name: c.name.clone(),
                    reason: c.status_reason.clone(),
                })
                .collect();
            let failed_json: Vec<PlanNameReason> = failed
                .iter()
                .map(|c| PlanNameReason {
                    name: c.name.clone(),
                    reason: c.status_reason.clone(),
                })
                .collect();

            let active_json = active.map(|a| PlanActiveJson {
                name: a.name.clone(),
                lifecycle: active_lifecycle.clone(),
            });

            emit_response(PlanStatusResponse {
                plan: PlanRef {
                    name: plan.name.clone(),
                    path: plan_path.display().to_string(),
                },
                counts: PlanCounts {
                    done: counts[&PlanStatus::Done],
                    in_progress: counts[&PlanStatus::InProgress],
                    pending: counts[&PlanStatus::Pending],
                    blocked: counts[&PlanStatus::Blocked],
                    failed: counts[&PlanStatus::Failed],
                    skipped: counts[&PlanStatus::Skipped],
                    total,
                },
                order: order_label,
                entries,
                in_progress: active_json,
                blocked: blocked_json,
                failed: failed_json,
                next_eligible: next_eligible.map(|e| e.name.clone()),
            });
        }
        OutputFormat::Text => print_plan_status_text(&PlanStatusView {
            plan: &plan,
            counts: &counts,
            active,
            active_lifecycle: active_lifecycle.as_deref(),
            blocked: &blocked,
            failed: &failed,
            next_eligible,
        }),
    }
    Ok(CliResult::Success)
}

/// All the slices `print_plan_status_text` needs. Bundled so the
/// function takes one `&PlanStatusView` instead of eight positional
/// arguments.
struct PlanStatusView<'a> {
    plan: &'a Plan,
    counts: &'a BTreeMap<PlanStatus, usize>,
    active: Option<&'a PlanChange>,
    active_lifecycle: Option<&'a str>,
    blocked: &'a [&'a PlanChange],
    failed: &'a [&'a PlanChange],
    next_eligible: Option<&'a PlanChange>,
}

/// Best-effort load of `<change_dir>/.metadata.yaml` to surface the
/// lifecycle state of the in-progress change. Missing metadata returns
/// `None` — status rendering treats it as "no change dir yet".
fn load_lifecycle_label(change_dir: &Path) -> Option<String> {
    if !ChangeMetadata::path(change_dir).exists() {
        return None;
    }
    ChangeMetadata::load(change_dir).ok().map(|m| m.status.to_string())
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct PlanEntryJson {
    name: String,
    status: String,
    depends_on: Vec<String>,
    sources: Vec<String>,
    status_reason: Option<String>,
    description: Option<String>,
    lifecycle: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    context: Vec<String>,
}

fn plan_entry_to_json(entry: &PlanChange, lifecycle: Option<String>) -> Value {
    serde_json::to_value(PlanEntryJson {
        name: entry.name.clone(),
        status: entry.status.to_string(),
        depends_on: entry.depends_on.clone(),
        sources: entry.sources.clone(),
        status_reason: entry.status_reason.clone(),
        description: entry.description.clone(),
        lifecycle,
        context: entry.context.clone(),
    })
    .expect("PlanEntryJson serialises")
}

fn print_plan_status_text(view: &PlanStatusView) {
    let counts = view.counts;
    let total: usize = counts.values().sum();
    println!("## Initiative: {}", view.plan.name);
    println!();
    println!();
    println!(
        "Progress: done {}, in-progress {}, pending {}, blocked {}, failed {}, skipped {} (total {total})",
        counts[&PlanStatus::Done],
        counts[&PlanStatus::InProgress],
        counts[&PlanStatus::Pending],
        counts[&PlanStatus::Blocked],
        counts[&PlanStatus::Failed],
        counts[&PlanStatus::Skipped],
    );

    if let Some(a) = view.active {
        let lifecycle_label = view.active_lifecycle.unwrap_or("<no change dir yet>");
        println!();
        println!("In progress: {} (lifecycle: {lifecycle_label})", a.name);
    }

    if !view.blocked.is_empty() {
        println!();
        println!("Blocked:");
        for c in view.blocked {
            let reason = c.status_reason.as_deref().unwrap_or("-");
            println!("  - {} (reason: {reason})", c.name);
        }
    }

    if !view.failed.is_empty() {
        println!();
        println!("Failed:");
        for c in view.failed {
            let reason = c.status_reason.as_deref().unwrap_or("-");
            println!("  - {} (reason: {reason})", c.name);
        }
    }

    println!();
    match view.next_eligible {
        Some(e) => println!("Next eligible: {}", e.name),
        None => println!("Next eligible: — (waiting on dependencies / all done)"),
    }
}

fn load_plan_for_write(ctx: &CommandContext) -> Result<(PathBuf, Plan), Error> {
    let plan_path = require_plan_file(&ctx.project_dir)?;
    let plan = Plan::load(&plan_path)?;
    Ok((plan_path, plan))
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct PlanRef {
    name: String,
    path: String,
}

fn plan_ref_from(plan: &Plan, plan_path: &Path) -> PlanRef {
    PlanRef {
        name: plan.name.clone(),
        path: plan_path.display().to_string(),
    }
}

/// Serialize a `PlanChange` into the on-the-wire kebab-case JSON shape
/// (matches the fields emitted by `plan status.entries[]`, minus the
/// `lifecycle` overlay which is a status-report concern).
fn plan_change_entry_json(entry: &PlanChange) -> Value {
    serde_json::to_value(entry).expect("PlanChange serialises as JSON")
}

fn run_initiative_create(
    ctx: &CommandContext, name: String, depends_on: Vec<String>, sources: Vec<String>,
    description: Option<String>, project: Option<String>, schema: Option<String>,
    context: Vec<String>,
) -> Result<CliResult, Error> {
    let (plan_path, mut plan) = load_plan_for_write(ctx)?;

    if let Some(ref proj) = project {
        match Registry::load(&ctx.project_dir) {
            Ok(Some(registry)) => {
                if !registry.projects.iter().any(|p| p.name == *proj) {
                    return Err(Error::Config(format!(
                        "--project '{proj}' does not match any project in registry.yaml"
                    )));
                }
            }
            Ok(None) => {
                return Err(Error::Config(
                    "--project was specified but no registry.yaml exists".to_string(),
                ));
            }
            Err(err) => return Err(err),
        }
    }

    let entry = PlanChange {
        name: name.clone(),
        project,
        schema,
        status: PlanStatus::Pending,
        depends_on,
        sources,
        context,
        description,
        status_reason: None,
    };

    plan.create(entry)?;
    plan.save(&plan_path)?;

    // `Plan::create` forces status to Pending and clears status_reason, so
    // the freshly-appended entry is always the tail of `plan.changes`.
    let created = plan.changes.last().expect("Plan::create appended an entry that is now missing");

    #[derive(Serialize)]
    #[serde(rename_all = "kebab-case")]
    struct PlanCreateResponse {
        plan: PlanRef,
        action: &'static str,
        entry: Value,
    }
    match ctx.format {
        OutputFormat::Json => emit_response(PlanCreateResponse {
            plan: plan_ref_from(&plan, &plan_path),
            action: "create",
            entry: plan_change_entry_json(created),
        }),
        OutputFormat::Text => {
            println!("Created plan entry '{name}' with status 'pending'.");
        }
    }
    Ok(CliResult::Success)
}

fn run_initiative_amend(
    ctx: &CommandContext, name: String, depends_on: Option<Vec<String>>,
    sources: Option<Vec<String>>, description: Option<String>, project: Option<String>,
    schema: Option<String>, context: Option<Vec<String>>,
) -> Result<CliResult, Error> {
    let (plan_path, mut plan) = load_plan_for_write(ctx)?;

    if let Some(ref proj) = project
        && !proj.is_empty()
    {
        match Registry::load(&ctx.project_dir) {
            Ok(Some(registry)) => {
                if !registry.projects.iter().any(|p| p.name == *proj) {
                    return Err(Error::Config(format!(
                        "--project '{proj}' does not match any project in registry.yaml"
                    )));
                }
            }
            Ok(None) => {
                return Err(Error::Config(
                    "--project was specified but no registry.yaml exists".to_string(),
                ));
            }
            Err(err) => return Err(err),
        }
    }

    let description_patch: Option<Option<String>> =
        description.map(|s| if s.is_empty() { None } else { Some(s) });
    let project_patch: Option<Option<String>> =
        project.map(|s| if s.is_empty() { None } else { Some(s) });
    let schema_patch: Option<Option<String>> =
        schema.map(|s| if s.is_empty() { None } else { Some(s) });

    let patch = PlanChangePatch {
        depends_on,
        sources,
        project: project_patch,
        schema: schema_patch,
        description: description_patch,
        context,
    };

    plan.amend(&name, patch)?;
    plan.save(&plan_path)?;

    let amended = plan.changes.iter().find(|c| c.name == name).expect("amended entry present");

    #[derive(Serialize)]
    #[serde(rename_all = "kebab-case")]
    struct PlanAmendResponse {
        plan: PlanRef,
        action: &'static str,
        entry: Value,
    }
    match ctx.format {
        OutputFormat::Json => emit_response(PlanAmendResponse {
            plan: plan_ref_from(&plan, &plan_path),
            action: "amend",
            entry: plan_change_entry_json(amended),
        }),
        OutputFormat::Text => {
            println!("Amended plan entry '{name}'.");
        }
    }
    Ok(CliResult::Success)
}

fn run_initiative_transition(
    ctx: &CommandContext, name: String, target: PlanStatus, reason: Option<String>,
) -> Result<CliResult, Error> {
    let (plan_path, mut plan) = load_plan_for_write(ctx)?;

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
    struct PlanTransitionResponse {
        plan: PlanRef,
        entry: PlanTransitionEntry,
    }
    #[derive(Serialize)]
    #[serde(rename_all = "kebab-case")]
    struct PlanTransitionEntry {
        name: String,
        status: String,
        status_reason: Option<String>,
    }
    match ctx.format {
        OutputFormat::Json => emit_response(PlanTransitionResponse {
            plan: plan_ref_from(&plan, &plan_path),
            entry: PlanTransitionEntry {
                name: entry.name.clone(),
                status: entry.status.to_string(),
                status_reason: entry.status_reason.clone(),
            },
        }),
        OutputFormat::Text => {
            println!("Transitioned '{name}': {} → {}.", old_status, entry.status);
        }
    }
    Ok(CliResult::Success)
}

fn run_initiative_archive(ctx: &CommandContext, force: bool) -> Result<CliResult, Error> {
    let plan_path = ctx.project_dir.join(".specify/plan.yaml");
    if !plan_path.exists() {
        return Err(Error::ArtifactNotFound {
            kind: "plan.yaml",
            path: plan_path,
        });
    }
    let archive_dir = ProjectConfig::archive_dir(&ctx.project_dir).join("plans");

    // Grab the plan name up-front so we can surface it in the
    // success payload even though `Plan::archive` only returns the
    // archived path.
    let plan_name = Plan::load(&plan_path)?.name;

    match Plan::archive(&plan_path, &archive_dir, force) {
        Ok((archived, archived_plans_dir)) => {
            match ctx.format {
                OutputFormat::Json => {
                    #[derive(Serialize)]
                    #[serde(rename_all = "kebab-case")]
                    struct PlanArchiveResponse {
                        archived: String,
                        archived_plans_dir: Option<String>,
                        plan: PlanArchiveName,
                    }
                    #[derive(Serialize)]
                    #[serde(rename_all = "kebab-case")]
                    struct PlanArchiveName {
                        name: String,
                    }
                    emit_response(PlanArchiveResponse {
                        archived: absolute_string(&archived),
                        archived_plans_dir: archived_plans_dir.as_deref().map(absolute_string),
                        plan: PlanArchiveName { name: plan_name },
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
                    struct PlanOutstandingError {
                        error: &'static str,
                        entries: Vec<String>,
                        exit_code: u8,
                    }
                    emit_response(PlanOutstandingError {
                        error: "plan-has-outstanding-work",
                        entries,
                        exit_code: CliResult::GenericFailure.code(),
                    });
                }
                OutputFormat::Text => {
                    eprintln!(
                        "Refusing to archive — outstanding non-terminal entries: {}. Re-run with --force to archive anyway.",
                        entries.join(", ")
                    );
                }
            }
            Ok(CliResult::GenericFailure)
        }
        Err(err) => Err(err),
    }
}

fn run_initiative_lock_acquire(ctx: &CommandContext, pid: Option<u32>) -> Result<CliResult, Error> {
    let our_pid = pid.unwrap_or_else(std::process::id);
    let acquired = PlanLockStamp::acquire(&ctx.project_dir, our_pid)?;
    Ok(emit_plan_lock_acquired(ctx.format, &acquired))
}

fn emit_plan_lock_acquired(format: OutputFormat, acquired: &PlanLockAcquired) -> CliResult {
    #[derive(Serialize)]
    #[serde(rename_all = "kebab-case")]
    struct LockAcquiredResponse {
        held: bool,
        pid: u32,
        already_held: bool,
        reclaimed_stale_pid: Option<u32>,
    }
    match format {
        OutputFormat::Json => emit_response(LockAcquiredResponse {
            held: true,
            pid: acquired.pid,
            already_held: acquired.already_held,
            reclaimed_stale_pid: acquired.reclaimed_stale_pid,
        }),
        OutputFormat::Text => {
            if acquired.already_held {
                println!("Lock already held by pid {}; re-stamped.", acquired.pid);
            } else {
                println!("Acquired plan lock for pid {}.", acquired.pid);
            }
            if let Some(stale) = acquired.reclaimed_stale_pid {
                println!("  (reclaimed stale stamp from pid {stale})");
            }
        }
    }
    CliResult::Success
}

fn run_initiative_lock_release(ctx: &CommandContext, pid: Option<u32>) -> Result<CliResult, Error> {
    let our_pid = pid.unwrap_or_else(std::process::id);
    let outcome = PlanLockStamp::release(&ctx.project_dir, our_pid)?;
    Ok(emit_plan_lock_released(ctx.format, our_pid, &outcome))
}

/// Mirrors the four [`PlanLockReleased`] outcomes onto the CLI
/// response. All four exit 0 — a mismatched holder is a warning, not
/// an error, per RFC-2 §"Driver Concurrency" (stale reclaim is the
/// self-heal path's job, not release's).
fn emit_plan_lock_released(
    format: OutputFormat, our_pid: u32, outcome: &PlanLockReleased,
) -> CliResult {
    match format {
        OutputFormat::Json => {
            #[derive(Serialize)]
            #[serde(rename_all = "kebab-case")]
            struct LockReleasedResponse {
                result: &'static str,
                pid: Option<u32>,
                #[serde(skip_serializing_if = "Option::is_none")]
                our_pid: Option<u32>,
            }
            let payload = match outcome {
                PlanLockReleased::Removed { pid } => LockReleasedResponse {
                    result: "removed",
                    pid: Some(*pid),
                    our_pid: None,
                },
                PlanLockReleased::WasAbsent => LockReleasedResponse {
                    result: "was-absent",
                    pid: None,
                    our_pid: None,
                },
                PlanLockReleased::HeldByOther { pid } => LockReleasedResponse {
                    result: "held-by-other",
                    pid: *pid,
                    our_pid: Some(our_pid),
                },
            };
            emit_response(payload);
        }
        OutputFormat::Text => match outcome {
            PlanLockReleased::Removed { pid } => {
                println!("Released plan lock held by pid {pid}.");
            }
            PlanLockReleased::WasAbsent => {
                println!("No plan lock to release.");
            }
            PlanLockReleased::HeldByOther { pid: Some(other) } => {
                eprintln!(
                    "warning: plan lock is held by pid {other}, not {our_pid}; not removing."
                );
            }
            PlanLockReleased::HeldByOther { pid: None } => {
                eprintln!(
                    "warning: plan lock contents are malformed; refusing to clobber (run the L2.G self-heal path)."
                );
            }
        },
    }
    CliResult::Success
}

fn run_initiative_lock_status(ctx: &CommandContext) -> Result<CliResult, Error> {
    let state = PlanLockStamp::status(&ctx.project_dir)?;
    Ok(emit_plan_lock_state(ctx.format, &state))
}

fn emit_plan_lock_state(format: OutputFormat, state: &PlanLockState) -> CliResult {
    #[derive(Serialize)]
    #[serde(rename_all = "kebab-case")]
    struct LockStateResponse {
        held: bool,
        pid: Option<u32>,
        stale: Option<bool>,
    }
    match format {
        OutputFormat::Json => emit_response(LockStateResponse {
            held: state.held,
            pid: state.pid,
            stale: state.stale,
        }),
        OutputFormat::Text => match state.pid {
            Some(pid) => {
                let is_stale = state.stale.unwrap_or(false);
                if is_stale {
                    println!("stale (pid {pid} no longer alive)");
                } else {
                    println!("held by pid {pid}");
                }
            }
            None => match state.stale {
                Some(true) => println!("stale (malformed lockfile contents)"),
                _ => println!("no lock"),
            },
        },
    }
    CliResult::Success
}
