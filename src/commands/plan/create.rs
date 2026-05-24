//! `specify plan create` handler. Composes the shared CLI arg
//! parsers in [`super::args`] with the domain authority-override
//! engine in [`specify_domain::change::mutate_authority_overrides`]
//! so the handler stays declarative.

use std::io::Write;

use serde::Serialize;
use specify_domain::change::{
    Divergence, Lifecycle, Plan, mutate_authority_overrides, refuse_orphan_authority_overrides,
};
use specify_domain::journal;
use specify_error::{Error, Result, is_kebab};

use super::args::{build_source_map, parse_authority_override_assigns};
use crate::cli::SourceArg;
use crate::context::Ctx;

/// `specify plan create <name> [--source ...] [--divergence-likely <slice>]... [--auto-review]`.
///
/// Scaffolds `plan.yaml` (workflow §The Plan), then stages every
/// `--divergence-likely <slice>` value onto the named slice's
/// `slices[].divergence` field (workflow §D5). The slice MUST already
/// exist in the plan being created — an unknown name short-circuits
/// with `plan-divergence-likely-unknown-slice` (`Error::Validation`,
/// exit 2). One `plan.propose.divergence` journal event fires per
/// applied slice, matching the post-`propose` happy path the
/// `/spec:plan` skill drives.
///
/// When `--auto-review` is set (workflow §D7), the plan is constructed
/// with `lifecycle: reviewed` *before* the single atomic
/// `plan.save` — there is never a transient `lifecycle: pending`
/// file on disk. The matching `plan.transition.reviewed` journal
/// event is appended in the same batched write as any
/// `plan.propose.divergence` events the same invocation produced;
/// validation failures (kebab-case name, orphan source key,
/// unknown `--divergence-likely` slice) refuse the create with or
/// without the flag and leave the journal untouched.
pub(super) fn create(
    ctx: &Ctx, name: String, sources: Vec<SourceArg>, divergence_likely: &[String],
    auto_review: bool, authority_override: &[String],
) -> Result<()> {
    if !is_kebab(&name) {
        return Err(Error::Diag {
            code: "change-name-not-kebab",
            detail: format!(
                "change: name `{name}` must be kebab-case \
                 (lowercase ascii, digits, single hyphens; no leading/trailing/doubled hyphens)"
            ),
        });
    }
    let source_map = build_source_map(sources)?;
    let plan_path = ctx.layout().plan_path();
    if plan_path.exists() {
        return Err(Error::Diag {
            code: "already-exists",
            detail: format!("refusing to overwrite existing plan at {}", plan_path.display()),
        });
    }

    let override_assigns = parse_authority_override_assigns(authority_override)?;

    let mut plan = Plan::init(&name, source_map)?;
    apply_divergence_likely(&mut plan, divergence_likely)?;
    // Route `--authority-override` through the shared mutation
    // helper used by `plan amend` so create and amend produce
    // byte-identical `plan.amend.authority-override` journal events
    // and share the unknown-slice gate. Empty `clears` / `clear_all`
    // slices keep the create path scoped to set-only semantics.
    let now = jiff::Timestamp::now();
    let plan_name = plan.name.clone();
    let override_events =
        mutate_authority_overrides(&mut plan, &plan_name, &override_assigns, &[], &[], now)?;
    // Re-run the orphan-source-key gate after the override
    // pre-seeding: `Plan::init` ran no validation against the
    // override map (it didn't exist yet) and `validate_plan` only
    // checks JSON Schema. The orphan check is the only workflow §D3
    // gate that fires on this code path.
    refuse_orphan_authority_overrides(&plan)?;
    if auto_review {
        plan.transition_lifecycle(Lifecycle::Reviewed)?;
    }
    plan.save(&plan_path)?;

    // Collect every journal event the invocation produced, then
    // hand the slice to `append_batch` so the post-save log write is
    // a single fsynced append. Either every event lands or none
    // does — `--auto-review`, `--divergence-likely`, and
    // `--authority-override` compose without a partial-state window
    // in the journal.
    let mut events: Vec<journal::Event> = divergence_likely
        .iter()
        .map(|slice| {
            journal::Event::new(
                now,
                journal::EventKind::PlanProposeDivergence {
                    plan_name: plan_name.clone(),
                    slice_name: slice.clone(),
                },
            )
        })
        .collect();
    if auto_review {
        events.push(journal::Event::new(
            now,
            journal::EventKind::PlanTransitionReviewed {
                plan_name: plan_name.clone(),
            },
        ));
    }
    events.extend(override_events);
    journal::append_batch(ctx.layout(), &events)?;

    ctx.write(
        &CreateBody {
            name,
            plan: plan_path.display().to_string(),
            lifecycle: plan.lifecycle,
        },
        write_create_text,
    )?;
    Ok(())
}

/// Stamp `divergence: likely` on every named slice in `plan`.
/// Rejects unknown slice names with `Error::validation_failed` —
/// `plan-divergence-likely-unknown-slice` (exit 2). Duplicate
/// occurrences of the same slice are idempotent (the field re-sets
/// to `Likely`).
fn apply_divergence_likely(plan: &mut Plan, slices: &[String]) -> Result<()> {
    for slice in slices {
        let entry = plan.entries.iter_mut().find(|e| &e.name == slice).ok_or_else(|| {
            Error::validation_failed(
                "plan-divergence-likely-unknown-slice",
                "--divergence-likely must reference a slice present in the plan",
                format!(
                    "no slice named '{slice}' in plan '{}'; add the slice (e.g. specify plan \
                     add {slice}) before staging divergence: likely",
                    plan.name
                ),
            )
        })?;
        entry.divergence = Some(Divergence::Likely);
    }
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct CreateBody {
    name: String,
    plan: String,
    /// Final plan-level lifecycle persisted to disk — `pending` for
    /// the default create, `reviewed` when `--auto-review` was set.
    /// Exposed in the JSON envelope so skill bodies and tests can
    /// branch on the on-disk state without re-reading `plan.yaml`.
    lifecycle: Lifecycle,
}

fn write_create_text(w: &mut dyn Write, body: &CreateBody) -> std::io::Result<()> {
    match body.lifecycle {
        Lifecycle::Pending => writeln!(w, "Initialised plan '{}' at {}.", body.name, body.plan),
        Lifecycle::Reviewed => writeln!(
            w,
            "Initialised plan '{}' at {} and stamped lifecycle: reviewed.",
            body.name, body.plan
        ),
    }
}
