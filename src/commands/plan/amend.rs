//! `specify plan amend` handler. Routes wholesale edits through
//! [`Plan::amend`], additive `--add-source` / `--remove-source`
//! through direct entry mutation, and authority-override flags
//! through the shared domain engine.

use specify_domain::change::{
    Divergence, EntryPatch, Patch, Plan, entry_mut, mutate_authority_overrides,
    refuse_orphan_authority_overrides,
};
use specify_domain::config::with_state;
use specify_domain::discovery::Discovery;
use specify_domain::evidence::ClaimKind;
use specify_domain::journal;
use specify_domain::schema::validate_plan;
use specify_error::{Error, Result};

use super::args::{
    bindings_from_args, load_discovery, parse_authority_override_assigns, parse_divergence,
    parse_slice_pair_args, parse_target_flag,
};
use super::entry::{Action, EntryBody, write_entry_text};
use super::{check_project, plan_ref};
use crate::cli::{AliasAssign, SliceSourceArg};
use crate::context::Ctx;

#[expect(
    clippy::too_many_arguments,
    reason = "plan amend's clap surface is the source of truth for the argument set; \
              the handler threads it through verbatim."
)]
pub(super) fn amend(
    ctx: &Ctx, name: String, depends_on: Option<Vec<String>>, sources: Option<Vec<SliceSourceArg>>,
    add_source: Vec<SliceSourceArg>, remove_source: Vec<String>, divergence: Option<&str>,
    description: Option<String>, project: Option<String>, target: Option<String>,
    context: Option<Vec<String>>, authority_override: &[String],
    clear_authority_override: &[String], clear_authority_overrides: &[String],
    add_alias: &[AliasAssign], remove_alias: &[AliasAssign],
) -> Result<()> {
    if let Some(proj) = &project
        && !proj.is_empty()
    {
        check_project(&ctx.project_dir, proj)?;
    }

    let divergence = divergence.map(parse_divergence).transpose()?;
    let override_sets = parse_authority_override_assigns(authority_override)?;
    let override_clears: Vec<(String, ClaimKind)> =
        parse_slice_pair_args::<ClaimKind>(clear_authority_override, "--clear-authority-override")?;
    let override_clear_all: Vec<String> = clear_authority_overrides.to_vec();
    let plan_path = ctx.layout().plan_path();
    // workflow §D6 — `--add-alias` / `--remove-alias` mutate
    // `discovery.md`, NOT `plan.yaml`. We apply them up-front so the
    // updated discovery feeds the subsequent `--sources` rewrite
    // path on the same invocation; the in-memory Discovery is also
    // the source of truth for the whole-document collision gate that
    // refuses the amend (with `discovery-alias-collision`, exit 2)
    // before any write hits disk.
    let discovery = apply_alias_edits(ctx, add_alias, remove_alias)?;
    let (body, journal_events) =
        with_state::<Plan, _, _>(ctx.layout(), "plan.yaml", move |plan| {
            // We materialise per-slice bindings here (rather than in
            // the dispatcher) so the slice-name resolution lines up
            // with the slice we're actually mutating. Aliases are
            // resolved against `discovery.md` before the binding
            // lands in memory.
            let sources_replace = sources
                .as_ref()
                .map(|v| bindings_from_args(v.clone(), &name, discovery.as_ref()))
                .transpose()?;
            let add_bindings = bindings_from_args(add_source.clone(), &name, discovery.as_ref())?;

            // Capture pre-amend divergence so the journal event's
            // `from` field carries the implicit-default `none` on the
            // first transition (workflow §Observability).
            let plan_name = plan.name.clone();
            let previous_divergence =
                plan.entries.iter().find(|e| e.name == name).and_then(|e| e.divergence);

            let patch = EntryPatch {
                depends_on: depends_on.clone(),
                sources: sources_replace,
                project: Patch::from_string_option(project.clone()),
                target: match target.clone() {
                    None => Patch::Keep,
                    Some(s) if s.is_empty() => Patch::Clear,
                    Some(s) => Patch::Set(parse_target_flag(&s)?),
                },
                description: Patch::from_string_option(description.clone()),
                context: context.clone(),
                divergence,
            };
            plan.amend(&name, patch)?;

            // Apply --add-source / --remove-source after the wholesale
            // `amend` so additive edits compose cleanly with a
            // simultaneous `--sources` replacement.
            if !add_bindings.is_empty() || !remove_source.is_empty() {
                let entry = entry_mut(plan, &plan_name, &name)?;
                for key in &remove_source {
                    let before = entry.sources.len();
                    entry.sources.retain(|b| b.key() != key.as_str());
                    if entry.sources.len() == before {
                        return Err(Error::Diag {
                            code: "plan-binding-not-found",
                            detail: format!(
                                "slice `{name}` has no source binding with key `{key}`"
                            ),
                        });
                    }
                }
                for binding in add_bindings {
                    entry.sources.push(binding);
                }
            }

            // Apply per-slice authority-override mutations. Order is
            // deterministic per workflow §D3: sets first (later
            // occurrences win on the same `(slice, kind)`), then
            // single-kind clears, then whole-map clears. The
            // mutations are gathered into journal events as we go so
            // the wire log matches the on-disk outcome exactly (and
            // so set-then-clear on the same kind emits only the
            // clear event).
            let now = jiff::Timestamp::now();
            let override_journal = mutate_authority_overrides(
                plan,
                &plan_name,
                &override_sets,
                &override_clears,
                &override_clear_all,
                now,
            )?;
            // Re-run the orphan-source-key gate after the override
            // mutations: `Plan::amend` validated the pre-mutation
            // state, and `validate_plan` only checks JSON Schema. The
            // orphan check is the only workflow §D3 gate that fires
            // on this code path.
            refuse_orphan_authority_overrides(plan)?;

            validate_plan(plan)?;
            let amended = plan
                .entries
                .iter()
                .find(|c| c.name == name)
                .ok_or_else(|| specify_domain::change::unknown_slice_err(&plan_name, &name))?;

            // Build the journal event only when --divergence flipped
            // the slice's `divergence` (workflow §Observability —
            // every operator transition is logged, including no-op
            // writes of the same value).
            let mut journal_events: Vec<journal::Event> = Vec::new();
            if let Some(to) = divergence {
                journal_events.push(journal::Event::new(
                    now,
                    journal::EventKind::PlanAmendDivergence {
                        plan_name,
                        slice_name: amended.name.clone(),
                        from: previous_divergence.unwrap_or(Divergence::None),
                        to,
                    },
                ));
            }
            journal_events.extend(override_journal);

            Ok((
                EntryBody {
                    plan: plan_ref(plan, &plan_path),
                    action: Action::Amend,
                    entry: amended.clone(),
                },
                journal_events,
            ))
        })?;
    journal::append_batch(ctx.layout(), &journal_events)?;

    ctx.write(&body, write_entry_text)?;
    Ok(())
}

/// Apply `--add-alias` / `--remove-alias` flag values to
/// `<project_dir>/discovery.md` and return the updated in-memory
/// document so the same amend invocation can keep using the alias
/// for subsequent `--sources` rewrites.
///
/// When neither flag was passed, returns the loaded discovery
/// unchanged (or `Ok(None)` when no `discovery.md` exists). When
/// flags are present but no `discovery.md` exists on disk, refuses
/// with `Error::Diag` (`discovery-not-found`) — the operator
/// expected to edit a file that isn't there.
///
/// Mutations apply in argument order: every `--add-alias` first,
/// then every `--remove-alias`. The whole-document collision gate
/// runs before the atomic write; any collision refuses the whole
/// amend (no partial state lands on disk). `discovery.md` is
/// written via [`Discovery::write_atomic`] so the file always
/// reflects either the pre- or post-mutation state.
fn apply_alias_edits(
    ctx: &Ctx, add_alias: &[AliasAssign], remove_alias: &[AliasAssign],
) -> Result<Option<Discovery>> {
    let layout = ctx.layout();
    let path = layout.discovery_path();
    let no_edits = add_alias.is_empty() && remove_alias.is_empty();

    if no_edits {
        return load_discovery(layout);
    }

    if !path.exists() {
        return Err(Error::Diag {
            code: "discovery-not-found",
            detail: format!(
                "--add-alias / --remove-alias require `{}` to exist; run `/spec:plan` to author \
                 the candidate inventory first",
                path.display()
            ),
        });
    }

    let mut discovery = Discovery::load(&path)?;
    for AliasAssign { candidate, alias } in add_alias {
        discovery.add_alias(candidate, alias)?;
    }
    for AliasAssign { candidate, alias } in remove_alias {
        discovery.remove_alias(candidate, alias)?;
    }
    // Catch pre-existing collisions when the operator only ran
    // --remove-alias; --add-alias already paid for itself.
    let collisions = discovery.check_alias_collisions();
    if !collisions.is_empty() {
        return Err(Discovery::collision_error(&collisions));
    }
    discovery.write_atomic(&path)?;
    Ok(Some(discovery))
}
