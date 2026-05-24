use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::str::FromStr;

use serde::Serialize;
use specify_domain::change::{
    Divergence, Entry, EntryPatch, Lifecycle, Patch, Plan, Severity, SliceAuthorityOverride,
    SliceSourceBinding, SourceBinding, Status, TargetRef, TargetRefParseError,
    authority_override_orphan_source_keys,
};
use specify_domain::config::with_state;
use specify_domain::discovery::{Discovery, DiscoveryResolveError};
use specify_domain::evidence::ClaimKind;
use specify_domain::journal::{self, AuthorityOverrideAction};
use specify_domain::schema::validate_plan;
use specify_error::{Error, Result, is_kebab};

use super::{Ref, check_project, plan_ref};
use crate::cli::{AliasAssign, AuthorityOverrideKindAssign, SliceSourceArg, SourceArg};
use crate::context::Ctx;

/// Validate `--source <key>=<adapter>:<binding>` arguments and
/// collapse them into the structured [`SourceBinding`] map
/// `Plan::init` expects. Refuses duplicate keys with the stable
/// `plan-source-duplicate-key` diagnostic.
///
/// Wire grammar (parsed in [`crate::cli::SourceArg::from_str`]):
///
/// - `<key>=<adapter>:<path>` → `SourceBinding { adapter, path: Some(_), value: None }`.
/// - `<key>=<adapter>:value:<literal>` → `SourceBinding { adapter, path: None, value: Some(_) }`.
pub fn build_source_map(sources: Vec<SourceArg>) -> Result<BTreeMap<String, SourceBinding>> {
    let mut map: BTreeMap<String, SourceBinding> = BTreeMap::new();
    for SourceArg {
        key,
        adapter,
        path,
        value,
    } in sources
    {
        if map.contains_key(&key) {
            return Err(Error::Diag {
                code: "plan-source-duplicate-key",
                detail: format!("duplicate key `{key}` in --source arguments"),
            });
        }
        map.insert(key, SourceBinding { adapter, path, value });
    }
    Ok(map)
}

/// Materialise CLI `--sources` / `--add-source` arguments into the
/// on-disk [`SliceSourceBinding`] shape, preferring the bare-string
/// shorthand when the candidate id equals the slice's name (RFC-25
/// §`Slice.sources`).
///
/// RFC-27 §D6 — when `discovery` is `Some(_)`, the operator-supplied
/// candidate value is resolved against the loaded `discovery.md` so
/// aliases rewrite to the canonical `id` before persisting. Unknown
/// tokens or alias collisions surface as `Error::validation_failed`
/// (exit 2) with the discriminants `discovery-candidate-unknown` and
/// `discovery-alias-collision` respectively. With `discovery` `None`
/// (no `discovery.md` on disk) the discovery-absent passthrough applies —
/// the supplied value is used verbatim.
fn binding_from_arg(
    arg: SliceSourceArg, slice_name: &str, discovery: Option<&Discovery>,
) -> Result<SliceSourceBinding> {
    let candidate = match arg.candidate {
        None => None,
        Some(value) => Some(resolve_candidate_token(&value, discovery)?),
    };
    Ok(match candidate {
        None => SliceSourceBinding::Bare(arg.key),
        Some(candidate) if candidate == slice_name => SliceSourceBinding::Bare(arg.key),
        Some(candidate) => SliceSourceBinding::Structured {
            key: arg.key,
            candidate,
        },
    })
}

/// Parse a CLI `--target <name@vN>` flag into a [`TargetRef`].
///
/// The schema regex on `plan.yaml.slices[].target` is the primary
/// gate; this helper is the matching gate at the CLI boundary so an
/// `Error::Argument` surfaces ahead of any plan I/O. The kebab
/// discriminant on the resulting argument-error is `plan-target-malformed`
/// per `DECISIONS.md §"Target adapter suffix policy"`.
fn parse_target_flag(raw: &str) -> Result<TargetRef> {
    TargetRef::from_str(raw).map_err(|err: TargetRefParseError| Error::Argument {
        flag: "--target",
        detail: format!(
            "{err}. Expected `<name>@v<version>` with kebab `<name>` and a non-negative integer `<version>` (e.g. `omnia@v1`). \
             Discriminant: plan-target-malformed."
        ),
    })
}

/// Map every CLI `--sources` / `--add-source` argument into the
/// on-disk binding shape against `slice_name`. Aliases are resolved
/// against `discovery` when present (RFC-27 §D6).
fn bindings_from_args(
    args: Vec<SliceSourceArg>, slice_name: &str, discovery: Option<&Discovery>,
) -> Result<Vec<SliceSourceBinding>> {
    args.into_iter().map(|a| binding_from_arg(a, slice_name, discovery)).collect()
}

/// Rewrite a `--sources <key>=<value>` candidate token to the
/// canonical `id` discovered in `discovery.md`.
///
/// When `discovery` is `None` (no `discovery.md` on disk), the
/// token round-trips unchanged — the legacy path predates RFC-27
/// §D6 and many tests operate without a discovery file.
///
/// # Errors
///
/// - [`Error::Validation`] / `discovery-alias-collision` when the
///   token resolves to more than one candidate (the document itself
///   is invalid).
/// - [`Error::Validation`] / `discovery-candidate-unknown` when no
///   candidate has a matching `id` or `aliases[]` entry.
fn resolve_candidate_token(token: &str, discovery: Option<&Discovery>) -> Result<String> {
    let Some(discovery) = discovery else {
        return Ok(token.to_string());
    };
    match discovery.resolve_candidate(token) {
        Ok(candidate) => Ok(candidate.id.clone()),
        Err(DiscoveryResolveError::Unknown { token }) => Err(Error::validation_failed(
            "discovery-candidate-unknown",
            "--sources <key>=<value> must resolve to a candidate in discovery.md",
            format!(
                "no candidate in discovery.md has an id or alias matching `{token}`; inspect \
                 discovery.md directly to review the inventory"
            ),
        )),
        Err(DiscoveryResolveError::Collision { token, candidates }) => {
            Err(Error::validation_failed(
                "discovery-alias-collision",
                "candidate id and aliases share a single namespace per discovery.md",
                format!(
                    "`{token}` resolves to multiple candidates in discovery.md: {}; run \
                 `specify slice validate` to enumerate every collision",
                    candidates.join(", ")
                ),
            ))
        }
    }
}

/// Best-effort load of `<project_dir>/discovery.md`. Returns
/// `Ok(None)` when the file is absent so the legacy plan-create
/// path (with no `discovery.md`) keeps working; propagates parse /
/// I/O errors otherwise.
fn load_discovery(layout: specify_domain::config::Layout<'_>) -> Result<Option<Discovery>> {
    let path = layout.discovery_path();
    if !path.exists() {
        return Ok(None);
    }
    Ok(Some(Discovery::load(&path)?))
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
    // Catch pre-existing collisions when the operator only ran --remove-alias; --add-alias already paid for itself.
    let collisions = discovery.check_alias_collisions();
    if !collisions.is_empty() {
        return Err(Discovery::collision_error(&collisions));
    }
    discovery.write_atomic(&path)?;
    Ok(Some(discovery))
}

/// Parse the `--divergence` flag value. `likely` / `accepted` /
/// `rejected` are wire-legal — RFC-27 §D5 widens the operator
/// surface so the CLI is the single writer of every variant
/// reachable on disk. The implicit default (absent on disk) has
/// no flag spelling; any other token — including `none` — falls
/// through to the catch-all and is rejected with the same
/// actionable hint.
fn parse_divergence(raw: &str) -> Result<Divergence> {
    match raw {
        "likely" => Ok(Divergence::Likely),
        "accepted" => Ok(Divergence::Accepted),
        "rejected" => Ok(Divergence::Rejected),
        other => Err(Error::Argument {
            flag: "--divergence",
            detail: format!(
                "`{other}` is not a valid --divergence value; expected `likely`, `accepted`, or \
                 `rejected`"
            ),
        }),
    }
}

/// Chunk a clap `num_args = 2` flag payload (`Vec<String>` of
/// interleaved `<slice>` and `<value>` values) into typed
/// `(slice, T)` pairs. The value half is parsed via `T`'s
/// `FromStr` impl, so the closed enum (`ClaimKind`) and the
/// composite assign (`AuthorityOverrideKindAssign`) share one
/// implementation.
fn parse_slice_pair_args<T>(raw: &[String], flag: &'static str) -> Result<Vec<(String, T)>>
where
    T: FromStr<Err = String>,
{
    let mut out = Vec::with_capacity(raw.len() / 2);
    for chunk in raw.chunks_exact(2) {
        let slice = chunk[0].clone();
        if slice.is_empty() {
            return Err(Error::Argument {
                flag,
                detail: format!("{flag} <slice> must be non-empty"),
            });
        }
        let value: T =
            chunk[1].parse().map_err(|detail: String| Error::Argument { flag, detail })?;
        out.push((slice, value));
    }
    Ok(out)
}

fn parse_authority_override_assigns(raw: &[String]) -> Result<Vec<(String, ClaimKind, String)>> {
    Ok(parse_slice_pair_args::<AuthorityOverrideKindAssign>(raw, "--authority-override")?
        .into_iter()
        .map(|(slice, a)| (slice, a.kind, a.source_key))
        .collect())
}

/// Refuse the whole CLI invocation if any flag references a slice
/// not present on `plan`. Runs before any mutation so the
/// `with_state` writer is never invoked with a partial result.
fn refuse_unknown_slices(
    plan: &Plan, plan_name: &str, set_map: &BTreeMap<(String, ClaimKind), String>,
    clear_set: &BTreeSet<(String, ClaimKind)>, clear_all_set: &BTreeSet<String>,
) -> Result<()> {
    let known: BTreeSet<&str> = plan.entries.iter().map(|e| e.name.as_str()).collect();
    let unknown = set_map
        .keys()
        .map(|(s, _)| s.as_str())
        .chain(clear_set.iter().map(|(s, _)| s.as_str()))
        .chain(clear_all_set.iter().map(String::as_str))
        .find(|s| !known.contains(s));
    if let Some(slice) = unknown {
        return Err(unknown_slice_err(plan_name, slice));
    }
    Ok(())
}

fn entry_mut<'a>(plan: &'a mut Plan, plan_name: &str, slice: &str) -> Result<&'a mut Entry> {
    plan.entries
        .iter_mut()
        .find(|e| e.name == slice)
        .ok_or_else(|| unknown_slice_err(plan_name, slice))
}

/// Build the batched `plan.amend.authority-override` event list
/// matching the on-disk outcome of the mutation walk above.
/// Set events are emitted only for survivors (sets not subsequently
/// cleared); per-kind Clear events are deduplicated across the
/// `--clear-authority-override` and `--clear-authority-overrides`
/// surfaces.
type AuthorityOverrideSortKey = (String, Option<String>, AuthorityOverrideAction);

fn emit_override_events(
    plan_name: &str, set_map: &BTreeMap<(String, ClaimKind), String>,
    clear_set: &BTreeSet<(String, ClaimKind)>, clear_all_set: &BTreeSet<String>,
    clear_all_emitted: &BTreeMap<String, Vec<ClaimKind>>, now: jiff::Timestamp,
) -> Vec<journal::Event> {
    let mut pending: Vec<(AuthorityOverrideSortKey, journal::Event)> = Vec::new();
    let mut record = |slice: &str,
                      action: AuthorityOverrideAction,
                      claim_kind: ClaimKind,
                      source_key: Option<&str>| {
        let claim_kind = Some(claim_kind.to_string());
        pending.push((
            (slice.to_string(), claim_kind.clone(), action),
            journal::Event::new(
                now,
                journal::EventKind::PlanAmendAuthorityOverride {
                    plan_name: plan_name.to_string(),
                    slice_name: slice.to_string(),
                    action,
                    claim_kind,
                    source_key: source_key.map(str::to_owned),
                },
            ),
        ));
    };
    for ((slice, kind), key) in set_map {
        if clear_set.contains(&(slice.clone(), *kind)) || clear_all_set.contains(slice) {
            continue;
        }
        record(slice, AuthorityOverrideAction::Set, *kind, Some(key.as_str()));
    }
    for (slice, kind) in clear_set {
        if clear_all_set.contains(slice)
            && clear_all_emitted.get(slice).is_some_and(|kinds| kinds.contains(kind))
        {
            continue;
        }
        record(slice, AuthorityOverrideAction::Clear, *kind, None);
    }
    for (slice, kinds) in clear_all_emitted {
        for kind in kinds {
            record(slice, AuthorityOverrideAction::Clear, *kind, None);
        }
    }
    // Final sort gives a byte-stable batched append regardless of
    // operator-issued flag order: `(slice, kind, action)` per the
    // Change 2.3 prompt's "stable order" rule.
    pending.sort_by(|(left, _), (right, _)| left.cmp(right));
    pending.into_iter().map(|(_, event)| event).collect()
}

/// Apply the full `--authority-override` / `--clear-authority-override`
/// / `--clear-authority-overrides` mutation set on `plan` and return
/// the matching `plan.amend.authority-override` journal events
/// (RFC-27 §D3). Order is deterministic:
///
/// 1. Sets — collapse duplicate `(slice, kind)` pairs to the last
///    value.
/// 2. Single-kind clears — remove the entry if present (no-op if
///    absent).
/// 3. Whole-map clears — wipe the slice's entire map; emit one
///    `Clear` event per kind that was present before the wipe.
///
/// Set-then-clear on the same `(slice, kind)` resolves to the
/// cleared state, and the journal records the clear (not the set)
/// to match the on-disk outcome.
///
/// Unknown slice names short-circuit with
/// `plan-authority-override-unknown-slice` (exit 2). Set events
/// sort deterministically by `(slice, kind)`; clear events follow
/// the same sort. Shared by `plan create` (with empty clears) and
/// `plan amend`, so both paths emit byte-identical journal events
/// for the same `(slice, kind)` set.
fn mutate_authority_overrides(
    plan: &mut Plan, plan_name: &str, sets: &[(String, ClaimKind, String)],
    clears: &[(String, ClaimKind)], clear_all: &[String], now: jiff::Timestamp,
) -> Result<Vec<journal::Event>> {
    let set_map: BTreeMap<(String, ClaimKind), String> =
        sets.iter().cloned().map(|(s, k, v)| ((s, k), v)).collect();
    let clear_set: BTreeSet<(String, ClaimKind)> = clears.iter().cloned().collect();
    let clear_all_set: BTreeSet<String> = clear_all.iter().cloned().collect();

    refuse_unknown_slices(plan, plan_name, &set_map, &clear_set, &clear_all_set)?;
    for ((slice, kind), key) in &set_map {
        entry_mut(plan, plan_name, slice)?.authority_override.by_kind.insert(*kind, key.clone());
    }
    for (slice, kind) in &clear_set {
        entry_mut(plan, plan_name, slice)?.authority_override.by_kind.remove(kind);
    }
    let mut clear_all_emitted: BTreeMap<String, Vec<ClaimKind>> = BTreeMap::new();
    for slice in &clear_all_set {
        let entry = entry_mut(plan, plan_name, slice)?;
        let kinds: Vec<ClaimKind> = entry.authority_override.by_kind.keys().copied().collect();
        entry.authority_override.by_kind.clear();
        clear_all_emitted.insert(slice.clone(), kinds);
    }

    Ok(emit_override_events(
        plan_name,
        &set_map,
        &clear_set,
        &clear_all_set,
        &clear_all_emitted,
        now,
    ))
}

/// Run [`authority_override_orphan_source_keys`] on `plan` and
/// short-circuit the CLI write with a single `Error::Validation`
/// envelope when any finding fires. Findings are emitted in the
/// deterministic order the domain helper produces (slice
/// declaration order, then claim-kind iteration order); the
/// envelope records each one as a [`ValidationSummary`].
///
/// This is the post-mutation gate that catches new orphan
/// overrides introduced by `--authority-override` on `plan create`
/// / `plan amend`. The `plan add` path's `Plan::create` already
/// re-runs `Plan::validate` (which folds in the same check) so it
/// doesn't need a separate call.
fn refuse_orphan_authority_overrides(plan: &Plan) -> Result<()> {
    let findings: Vec<_> = authority_override_orphan_source_keys(&plan.entries)
        .into_iter()
        .filter(|f| f.level == Severity::Error)
        .collect();
    if findings.is_empty() {
        return Ok(());
    }
    let results: Vec<specify_error::ValidationSummary> = findings
        .iter()
        .map(|f| specify_error::ValidationSummary {
            status: specify_error::ValidationStatus::Fail,
            rule_id: f.code.to_string(),
            rule: "slice authority-override must reference a bound source key".to_string(),
            detail: Some(f.message.clone()),
        })
        .collect();
    Err(Error::Validation { results })
}

fn unknown_slice_err(plan_name: &str, slice: &str) -> Error {
    Error::validation_failed(
        "plan-authority-override-unknown-slice",
        "--authority-override / --clear-authority-override(s) must reference a slice present in \
         the plan",
        format!(
            "no slice named '{slice}' in plan '{plan_name}'; add the slice (e.g. specify plan add \
             {slice}) before authoring authority-override entries"
        ),
    )
}

/// `specify plan create <name> [--source ...] [--divergence-likely <slice>]... [--auto-review]`.
///
/// Scaffolds `plan.yaml` (RFC-25 §The Plan), then stages every
/// `--divergence-likely <slice>` value onto the named slice's
/// `slices[].divergence` field (RFC-27 §D5). The slice MUST already
/// exist in the plan being created — an unknown name short-circuits
/// with `plan-divergence-likely-unknown-slice` (`Error::Validation`,
/// exit 2). One `plan.propose.divergence` journal event fires per
/// applied slice, matching the post-`propose` happy path the
/// `/spec:plan` skill drives.
///
/// When `--auto-review` is set (RFC-27 §D7), the plan is constructed
/// with `lifecycle: reviewed` *before* the single atomic
/// `plan.save` — there is never a transient `lifecycle: pending`
/// file on disk. The matching `plan.transition.reviewed` journal
/// event is appended in the same batched write as any
/// `plan.propose.divergence` events the same invocation produced;
/// validation failures (kebab-case name, orphan source key,
/// unknown `--divergence-likely` slice) refuse the create with or
/// without the flag and leave the journal untouched.
///
/// NOTE: RFC-27 §D7 names a `plan.create` event as the first row
/// of the batched append, but no such variant exists in
/// [`specify_domain::journal::EventKind`] today (and the existing
/// `plan create` path has never written one). Introducing it would
/// change the event sequence emitted by the two-call path
/// (`create` → `transition reviewed`) outside Change 2.1's scope,
/// so this handler emits only `plan.transition.reviewed` under
/// `--auto-review`. Downstream consumers see the same event
/// sequence as the existing two-call path.
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
    // and share the unknown-slice gate. Empty `clears` /
    // `clear_all` slices keep the create path scoped to set-only
    // semantics.
    let now = jiff::Timestamp::now();
    let plan_name = plan.name.clone();
    let override_events =
        mutate_authority_overrides(&mut plan, &plan_name, &override_assigns, &[], &[], now)?;
    // Re-run the orphan-source-key gate after the override
    // pre-seeding: `Plan::init` ran no validation against the
    // override map (it didn't exist yet) and `validate_plan` only
    // checks JSON Schema. The orphan check is the only RFC-27
    // §D3 gate that fires on this code path.
    refuse_orphan_authority_overrides(&plan)?;
    if auto_review {
        // Single atomic write below carries `lifecycle: reviewed`
        // directly; readers never observe a transient `pending` plan
        // under --auto-review (RFC-27 §D7).
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

pub(super) fn add(
    ctx: &Ctx, name: &str, depends_on: Vec<String>, sources: Vec<SliceSourceArg>,
    description: Option<String>, project: Option<String>, target: Option<String>,
    context: Vec<String>, authority_override: &[AuthorityOverrideKindAssign],
) -> Result<()> {
    if let Some(proj) = &project {
        check_project(&ctx.project_dir, proj)?;
    }

    // RFC-27 §D6 — resolve `--sources <key>=<alias>` to the
    // canonical candidate `id` before persisting; the on-disk
    // `plan.yaml.slices[].sources[].candidate` always carries the
    // canonical id. Absence of `discovery.md` short-circuits to the
    // legacy (verbatim) path so existing tests and pre-RFC-27
    // projects continue to work.
    let discovery = load_discovery(ctx.layout())?;
    let sources = bindings_from_args(sources, name, discovery.as_ref())?;
    let authority_override_map = SliceAuthorityOverride {
        by_kind: authority_override
            .iter()
            .map(|a| (a.kind, a.source_key.clone()))
            .collect::<BTreeMap<_, _>>(),
    };
    let target = target.map(|raw| parse_target_flag(&raw)).transpose()?;
    let entry = Entry {
        name: name.to_string(),
        project,
        target,
        status: Status::Pending,
        depends_on,
        sources,
        context,
        description,
        divergence: None,
        authority_override: authority_override_map,
    };
    let plan_path = ctx.layout().plan_path();
    let (body, override_events) =
        with_state::<Plan, _, _>(ctx.layout(), "plan.yaml", move |plan| {
            plan.create(entry)?;
            validate_plan(plan)?;
            let plan_name = plan.name.clone();
            let now = jiff::Timestamp::now();
            // Route the seeded overrides through the shared writer
            // (no clears on the add path) so all three handlers emit
            // identically-shaped, identically-sorted Set events.
            let created_entry = entry_mut(plan, &plan_name, name)?.clone();
            let set_map: BTreeMap<(String, ClaimKind), String> = created_entry
                .authority_override
                .by_kind
                .iter()
                .map(|(kind, key)| ((created_entry.name.clone(), *kind), key.clone()))
                .collect();
            let events = emit_override_events(
                &plan_name,
                &set_map,
                &BTreeSet::new(),
                &BTreeSet::new(),
                &BTreeMap::new(),
                now,
            );
            Ok((
                EntryBody {
                    plan: plan_ref(plan, &plan_path),
                    action: Action::Create,
                    entry: created_entry,
                },
                events,
            ))
        })?;

    journal::append_batch(ctx.layout(), &override_events)?;
    ctx.write(&body, write_entry_text)?;
    Ok(())
}

#[expect(
    clippy::too_many_arguments,
    reason = "plan amend's clap surface is the source of truth for the argument set; threading it through the handler avoids stuffing the bag into a struct just for clippy."
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
    // RFC-27 §D6 — `--add-alias` / `--remove-alias` mutate
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
            // first transition (RFC-25 §Observability).
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

            // Apply per-slice authority-override mutations. Order
            // is deterministic per RFC-27 §D3: sets first (later
            // occurrences win on the same `(slice, kind)`), then
            // single-kind clears, then whole-map clears. The
            // mutations are gathered into journal events as we go
            // so the wire log matches the on-disk outcome
            // exactly (and so set-then-clear on the same kind
            // emits only the clear event).
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
            // state, and `validate_plan` only checks JSON Schema.
            // The orphan check is the only RFC-27 §D3 gate that
            // fires on this code path.
            refuse_orphan_authority_overrides(plan)?;

            validate_plan(plan)?;
            let amended = plan
                .entries
                .iter()
                .find(|c| c.name == name)
                .ok_or_else(|| unknown_slice_err(&plan_name, &name))?;

            // Build the journal event only when --divergence flipped
            // the slice's `divergence` (RFC-25 §Observability — every
            // operator transition is logged, including no-op writes
            // of the same value).
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

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
enum Action {
    Create,
    Amend,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct EntryBody {
    plan: Ref,
    action: Action,
    entry: Entry,
}

fn write_entry_text(w: &mut dyn Write, body: &EntryBody) -> std::io::Result<()> {
    let name = &body.entry.name;
    match body.action {
        Action::Create => writeln!(w, "Created plan entry '{name}' with status 'pending'."),
        Action::Amend => writeln!(w, "Amended plan entry '{name}'."),
    }
}
