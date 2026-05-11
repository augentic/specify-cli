use std::io::Write;

use chrono::Utc;
use serde::Serialize;
use specify_capability::ChangeBrief;
use specify_change::{Finding, Plan, Severity, Status};
use specify_config::{LayoutExt, with_existing_state};
use specify_error::{Error, Result};
use specify_registry::Registry;

use super::{PlanRef, display, plan_ref, require_file};
use crate::cli::SourceArg;
use crate::context::Ctx;
use crate::output::{Render, Validation};

pub(super) fn create(ctx: &Ctx, name: String, sources: Vec<SourceArg>) -> Result<()> {
    let plan_path = ctx.project_dir.layout().plan_path();
    if plan_path.exists() {
        return Err(Error::Diag {
            code: "plan-already-exists",
            detail: format!(
                "plan already exists at {}; run `specify change plan archive` first",
                plan_path.display()
            ),
        });
    }

    let mut source_map: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();
    for SourceArg { key, value } in sources {
        if source_map.contains_key(&key) {
            return Err(Error::Diag {
                code: "plan-source-duplicate-key",
                detail: format!("duplicate key `{key}` in --source arguments"),
            });
        }
        source_map.insert(key, value);
    }

    let plan = Plan::init(&name, source_map)?;
    // `with_state` is for load → mutate → save; `create` writes a fresh plan
    // and the pre-existence check above is the documented contract.
    plan.save(&plan_path)?;

    ctx.out().write(&CreateBody {
        plan: PlanRef {
            name,
            path: plan_path,
        },
    })?;
    Ok(())
}

pub(super) fn validate(ctx: &Ctx) -> Result<()> {
    let plan_path = require_file(&ctx.project_dir)?;
    let plan = Plan::load(&plan_path)?;
    let slices_dir = ctx.project_dir.layout().slices_dir();

    let (registry, registry_err) = match Registry::load(&ctx.project_dir) {
        Ok(reg) => (reg, None),
        Err(err) => (None, Some(err)),
    };
    let mut results = plan.validate(Some(&slices_dir), registry.as_ref());
    if let Some(err) = registry_err {
        results.push(Finding {
            level: Severity::Error,
            code: "registry-shape",
            message: err.to_string(),
            entry: None,
        });
    }
    if let Some(ref reg) = registry {
        let workspace_base = ctx.project_dir.layout().specify_dir().join("workspace");
        for rp in &reg.projects {
            let slot_project_yaml =
                workspace_base.join(&rp.name).join(".specify").join("project.yaml");
            if slot_project_yaml.exists()
                && let Ok(content) = std::fs::read_to_string(&slot_project_yaml)
                && let Ok(config) = serde_saphyr::from_str::<serde_json::Value>(&content)
                && let Some(slot_capability) = config.get("capability").and_then(|v| v.as_str())
                && slot_capability != rp.capability
            {
                results.push(Finding {
                    level: Severity::Warning,
                    code: "capability-mismatch-workspace",
                    message: format!(
                        "workspace clone '{}' has capability '{}' but registry declares '{}'; \
                         the clone's project.yaml is authoritative at execution time",
                        rp.name, slot_capability, rp.capability
                    ),
                    entry: None,
                });
            }
        }
    }

    let has_errors = results.iter().any(|r| matches!(r.level, Severity::Error));
    let rows: Vec<FindingRow<'_>> = results.iter().map(FindingRow::from).collect();
    ctx.out().write(&PlanValidateBody {
        plan: PlanRef {
            name: plan.name,
            path: plan_path,
        },
        validation: Validation { results: rows },
        passed: !has_errors,
    })?;
    if has_errors { Err(Error::PlanStructural) } else { Ok(()) }
}

pub(super) fn next(ctx: &Ctx) -> Result<()> {
    let plan_path = require_file(&ctx.project_dir)?;
    let plan = Plan::load(&plan_path)?;
    let slices_dir = ctx.project_dir.layout().slices_dir();

    let results = plan.validate(Some(&slices_dir), None);
    if results.iter().any(|r| matches!(r.level, Severity::Error)) {
        return Err(Error::PlanStructural);
    }

    let body = if let Some(active) = plan.entries.iter().find(|c| c.status == Status::InProgress) {
        NextBody {
            reason: Some("in-progress".into()),
            active: Some(active.name.clone()),
            ..NextBody::default()
        }
    } else if let Some(entry) = plan.next_eligible() {
        NextBody {
            next: Some(entry.name.clone()),
            project: entry.project.clone(),
            capability: entry.capability.clone(),
            description: entry.description.clone(),
            sources: Some(entry.sources.clone()),
            ..NextBody::default()
        }
    } else {
        let all_terminal =
            plan.entries.iter().all(|c| matches!(c.status, Status::Done | Status::Skipped));
        let reason = if all_terminal { "all-done" } else { "stuck" };
        NextBody {
            reason: Some(reason.into()),
            ..NextBody::default()
        }
    };
    ctx.out().write(&body)?;
    Ok(())
}

pub(super) fn transition(
    ctx: &Ctx, name: String, target: Status, reason: Option<String>,
) -> Result<()> {
    let plan_path = ctx.layout().plan_path();
    let body = with_existing_state::<Plan, _, _>(ctx.layout(), "plan.yaml", move |plan| {
        let old_status = plan
            .entries
            .iter()
            .find(|c| c.name == name)
            .ok_or_else(|| Error::Diag {
                code: "plan-entry-not-found",
                detail: format!("no slice named '{name}' in plan"),
            })?
            .status;

        plan.transition(&name, target, reason.as_deref())?;

        let entry =
            plan.entries.iter().find(|c| c.name == name).expect("transitioned entry present");
        Ok(TransitionBody {
            plan: plan_ref(plan, &plan_path),
            entry: TransitionRow {
                name: entry.name.clone(),
                status: entry.status,
                status_reason: entry.status_reason.clone(),
            },
            previous_status: old_status,
        })
    })?;
    ctx.out().write(&body)?;
    Ok(())
}

pub(super) fn archive(ctx: &Ctx, force: bool) -> Result<()> {
    let layout = ctx.project_dir.layout();
    let plan_path = layout.plan_path();
    if !plan_path.exists() {
        return Err(Error::ArtifactNotFound {
            kind: "plan.yaml",
            path: plan_path,
        });
    }
    let archive_dir = layout.archive_dir().join("plans");
    let brief_path = ChangeBrief::path(&ctx.project_dir);
    let plan_name = Plan::load(&plan_path)?.name;

    let (archived, archived_plans_dir) =
        Plan::archive(&plan_path, &brief_path, &archive_dir, force, Utc::now())?;
    ctx.out().write(&ArchiveBody {
        archived: display(&archived),
        archived_plans_dir: archived_plans_dir.as_deref().map(display),
        plan: ArchivedPlan { name: plan_name },
    })?;
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct CreateBody {
    plan: PlanRef,
}

impl Render for CreateBody {
    fn render_text(&self, w: &mut dyn Write) -> std::io::Result<()> {
        writeln!(w, "Initialised plan '{}' at {}.", self.plan.name, display(&self.plan.path))
    }
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct PlanValidateBody<'a> {
    plan: PlanRef,
    #[serde(flatten)]
    validation: Validation<FindingRow<'a>>,
    passed: bool,
}

impl Render for PlanValidateBody<'_> {
    fn render_text(&self, w: &mut dyn Write) -> std::io::Result<()> {
        if self.validation.results.is_empty() {
            return writeln!(w, "Plan OK");
        }
        self.validation.render_text(w)
    }
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct FindingRow<'a> {
    level: FindingLevel,
    code: &'static str,
    entry: &'a Option<String>,
    message: &'a str,
}

#[derive(Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum FindingLevel {
    Error,
    Warning,
}

impl<'a> From<&'a Finding> for FindingRow<'a> {
    fn from(finding: &'a Finding) -> Self {
        let level = match finding.level {
            Severity::Error => FindingLevel::Error,
            Severity::Warning => FindingLevel::Warning,
        };
        Self {
            level,
            code: finding.code,
            entry: &finding.entry,
            message: &finding.message,
        }
    }
}

impl Render for FindingRow<'_> {
    fn render_text(&self, w: &mut dyn Write) -> std::io::Result<()> {
        let label = if self.level == FindingLevel::Error { "ERROR  " } else { "WARNING" };
        let entry_col = self.entry.as_ref().map_or_else(String::new, |e| format!("[{e}]"));
        writeln!(w, "{label} {:<32} {:<24} {}", self.code, entry_col, self.message)
    }
}

#[derive(Serialize, Default)]
#[serde(rename_all = "kebab-case")]
struct NextBody {
    next: Option<String>,
    reason: Option<String>,
    active: Option<String>,
    project: Option<String>,
    capability: Option<String>,
    description: Option<String>,
    sources: Option<Vec<String>>,
}

impl Render for NextBody {
    fn render_text(&self, w: &mut dyn Write) -> std::io::Result<()> {
        if let Some(active) = &self.active {
            writeln!(w, "Active change in progress: {active}")
        } else if let Some(name) = &self.next {
            writeln!(w, "{name}")
        } else if self.reason.as_deref() == Some("all-done") {
            writeln!(w, "All changes done.")
        } else {
            writeln!(
                w,
                "No eligible changes \u{2014} remaining entries are blocked, failed, or waiting on unmet dependencies."
            )
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct TransitionBody {
    plan: PlanRef,
    entry: TransitionRow,
    #[serde(skip)]
    previous_status: Status,
}

impl Render for TransitionBody {
    fn render_text(&self, w: &mut dyn Write) -> std::io::Result<()> {
        writeln!(
            w,
            "Transitioned '{}': {} \u{2192} {}.",
            self.entry.name, self.previous_status, self.entry.status
        )
    }
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct TransitionRow {
    name: String,
    status: Status,
    status_reason: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct ArchiveBody {
    archived: String,
    archived_plans_dir: Option<String>,
    plan: ArchivedPlan,
}

impl Render for ArchiveBody {
    fn render_text(&self, w: &mut dyn Write) -> std::io::Result<()> {
        match &self.archived_plans_dir {
            Some(dir) => {
                writeln!(w, "Archived plan to {}. Working directory moved to {dir}.", self.archived)
            }
            None => writeln!(w, "Archived plan to {}.", self.archived),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct ArchivedPlan {
    name: String,
}
