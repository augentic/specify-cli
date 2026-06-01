//! `specrun plan propose` handler — plan-time lead reconciliation
//! (RFC-29 D2; DECISIONS.md §"Lead reconciliation (D2)").
//!
//! Two mutually exclusive modes wrap the agent-led reconciliation kernel
//! that lives in `crates/workflow/src/change/plan/core/propose.rs`:
//!
//! - `--dry-run` is read-only. It requires a `plan.yaml`, reads the
//!   surveyed `discovery.md` lead inventory and the resolved project
//!   topology, and emits the `kind: request` envelope
//!   ([`ProposalRequest`]) for the agent to group. `--format json`
//!   prints the schema-valid envelope verbatim; nothing is written and
//!   no journal event fires.
//! - `--from <response.json>` is the only writer. It schema-gates the
//!   raw response bytes, deserialises the agent's grouping, **re-reads**
//!   `discovery.md` and the topology (never trusting a prior dry-run
//!   snapshot), projects the response onto `plan.yaml.slices[]` through
//!   [`Plan::propose_from`] under the atomic [`with_state`] write loop,
//!   and — only after the write commits — emits the paired
//!   `plan.reconcile.agent` + `plan.reconcile.completed` journal events
//!   in one batched append.
//!
//! Passing neither mode fails with `plan-propose-mode-required`
//! (exit 2); the clap layer rejects passing both.

use std::io::Write;
use std::path::Path;

use jiff::Timestamp;
use serde::Serialize;
use specify_error::{Error, Result};
use specify_model::discovery::Discovery;
use specify_workflow::change::{
    Plan, ProjectRef, ProposalRequest, ProposalResponse, ProposeOutcome, build_request,
    resolve_topology,
};
use specify_workflow::config::{ProjectConfig, with_state};
use specify_workflow::journal::{self, Event, EventKind};
use specify_workflow::schema::validate_proposal_json;

use super::{Ref, cli, plan_ref, require_file};
use crate::runtime::context::Ctx;

/// Run `specrun plan propose --dry-run | --from <response.json>`.
///
/// # Errors
///
/// - `plan-propose-mode-required` (exit 2) when neither `--dry-run` nor
///   `--from` is set.
/// - propagates every `plan-reconcile-*` projection error, the
///   `proposal-schema` gate failure, response read / parse failures,
///   and topology-resolution errors.
pub(super) fn propose(ctx: &Ctx, args: cli::ProposeArgs) -> Result<()> {
    match (args.dry_run, args.from) {
        (true, None) => dry_run(ctx),
        (false, Some(path)) => from(ctx, &path),
        // The clap `conflicts_with` guard makes `(true, Some(_))`
        // unreachable; return the mode error rather than risk a panic.
        (false, None) | (true, Some(_)) => Err(Error::validation_failed(
            "plan-propose-mode-required",
            "propose requires exactly one of --dry-run or --from",
            "pass exactly one of --dry-run or --from",
        )),
    }
}

/// `--dry-run`: emit the `kind: request` reconciliation envelope. Reads
/// `discovery.md` + topology; writes nothing.
fn dry_run(ctx: &Ctx) -> Result<()> {
    require_file(&ctx.project_dir)?;
    let discovery = load_discovery(ctx)?;
    let topology = load_topology(ctx)?;
    let request = build_request(&discovery, &topology)?;
    ctx.write(&request, write_request_text)
}

/// `--from`: schema-gate and project the agent response onto
/// `plan.yaml.slices[]`, then emit the paired reconciliation events.
fn from(ctx: &Ctx, response_path: &Path) -> Result<()> {
    let plan_path = require_file(&ctx.project_dir)?;
    let raw = read_response(response_path)?;

    // Schema gate on the raw bytes first: it enforces the kebab
    // patterns, uniqueItems, and kind/version consts the typed DTO does
    // not, so it must run before the structural deserialise.
    validate_proposal_json(&raw)?;
    let response: ProposalResponse = serde_json::from_str(&raw).map_err(|err| {
        Error::validation_failed(
            "plan-propose-response-parse",
            "the --from response deserialises as a reconciliation response",
            format!("failed to parse response envelope: {err}"),
        )
    })?;

    // Re-read the catalog and topology every invocation — `--from`
    // never trusts a prior dry-run snapshot.
    let discovery = load_discovery(ctx)?;
    let topology = load_topology(ctx)?;

    // The projection runs inside the atomic write loop: `propose_from`
    // replaces `plan.entries`, `with_state` writes `plan.yaml` on Ok and
    // rolls back on any Err.
    let projected = with_state::<Plan, _, _>(ctx.layout(), "plan.yaml", move |plan| {
        let outcome = plan.propose_from(response, &discovery, &topology)?;
        Ok(Projected {
            plan: plan_ref(plan, &plan_path),
            outcome,
        })
    })?;

    // Only after the write commits: emit both events in one batch.
    emit_reconcile_events(ctx, &projected)?;

    ctx.write(&summary(projected), write_summary_text)
}

/// Successful projection carried out of the [`with_state`] write loop so
/// the journal events and the summary can be built only after the atomic
/// `plan.yaml` write commits.
struct Projected {
    plan: Ref,
    outcome: ProposeOutcome,
}

/// `--from` success summary. `--format json` emits this verbatim.
#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct ProposeSummary {
    plan: Ref,
    slice_names: Vec<String>,
    slice_count: usize,
    scope_count: usize,
}

/// Emit the paired `plan.reconcile.agent` then `plan.reconcile.completed`
/// events in one batched, fsynced append — reached only after the
/// `plan.yaml` write has committed.
fn emit_reconcile_events(ctx: &Ctx, projected: &Projected) -> Result<()> {
    let slice_count = projected.outcome.slice_names.len();
    let events = [
        Event::new(
            Timestamp::now(),
            EventKind::PlanReconcileAgent {
                plan_name: projected.plan.name.clone(),
                scopes: projected.outcome.scopes.clone(),
                slice_count,
            },
        ),
        Event::new(
            Timestamp::now(),
            EventKind::PlanReconcileCompleted {
                plan_name: projected.plan.name.clone(),
                slice_count,
                slice_names: projected.outcome.slice_names.clone(),
            },
        ),
    ];
    journal::append_batch(ctx.layout(), &events)
}

/// Build the `--from` response summary from a committed projection.
fn summary(projected: Projected) -> ProposeSummary {
    ProposeSummary {
        slice_count: projected.outcome.slice_names.len(),
        scope_count: projected.outcome.scopes.len(),
        slice_names: projected.outcome.slice_names,
        plan: projected.plan,
    }
}

/// Load `discovery.md`, or an empty inventory when the file is absent so
/// the catalog assembly raises `plan-reconcile-empty-catalog` rather than
/// an I/O error on a never-surveyed plan.
fn load_discovery(ctx: &Ctx) -> Result<Discovery> {
    let path = ctx.layout().discovery_path();
    if path.exists() { Discovery::load(&path) } else { Discovery::parse("") }
}

/// Resolve the project topology the request embeds and the response binds
/// to — the committed `.specify/topology.lock` projection for a hub
/// (RFC-36), or the sole project synthesised from `project.yaml`.
fn load_topology(ctx: &Ctx) -> Result<Vec<ProjectRef>> {
    let config = ProjectConfig::load(&ctx.project_dir)?;
    resolve_topology(&config, &ctx.project_dir)
}

/// Read the `--from` response file, mapping a missing file to an exit-2
/// validation error rather than a generic I/O failure.
fn read_response(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            Error::validation_failed(
                "plan-propose-response-not-found",
                "the --from response file must exist",
                format!("no response file at {}", path.display()),
            )
        } else {
            Error::Io(err)
        }
    })
}

fn write_request_text(w: &mut dyn Write, body: &ProposalRequest) -> std::io::Result<()> {
    writeln!(w, "projects:")?;
    for project in &body.projects {
        writeln!(w, "  - {} ({})", project.name, project.target)?;
    }
    writeln!(w, "leads:")?;
    for lead in &body.leads {
        writeln!(w, "  - {}/{}: {}", lead.source, lead.lead, lead.synopsis)?;
    }
    Ok(())
}

fn write_summary_text(w: &mut dyn Write, body: &ProposeSummary) -> std::io::Result<()> {
    writeln!(w, "plan: {}", body.plan.name)?;
    writeln!(w, "path: {}", body.plan.path)?;
    writeln!(w, "slice-count: {}", body.slice_count)?;
    writeln!(w, "scope-count: {}", body.scope_count)?;
    if body.slice_names.is_empty() {
        writeln!(w, "slices: (none)")?;
    } else {
        writeln!(w, "slices: {}", body.slice_names.join(", "))?;
    }
    Ok(())
}
