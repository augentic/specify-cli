//! Lead-reconciliation envelope DTOs and the plan-time `propose`
//! domain core (RFC-29 D2).
//!
//! `specrun plan propose` wraps agent-led lead reconciliation in a
//! CLI-owned projection kernel. The wire contract is a single
//! envelope discriminated by a closed `kind: request | response`,
//! validated against `schemas/discovery/proposal.schema.json`
//! ([`crate::schema::validate_proposal_json`]). This module owns the
//! serde DTOs for both kinds plus the deterministic, filesystem-free
//! assembly the kernel runs:
//!
//! - [`build_request`] turns a surveyed `discovery.md` lead inventory
//!   plus a resolved project topology into the `kind: request`
//!   envelope the agent groups.
//! - [`build_catalog`] distils the same inventory into a
//!   [`LeadCatalog`] of `(source-key, lead-id)` identities, the
//!   membership oracle the response-validation kernel (a later chunk)
//!   checks every agent-supplied source binding against.
//! - [`resolve_topology`] normalises persisted project configuration
//!   (a platform hub's `registry.yaml`, or the sole project synthesised
//!   from `project.yaml`) into the envelope-local [`ProjectRef`] list.
//!
//! `build_request` / `build_catalog` are pure so they unit-test without
//! a temp project; the only filesystem access lives in
//! [`resolve_topology`], which resolves the regular project's target
//! adapter to its canonical `name@vN` ref.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;

use petgraph::algo::tarjan_scc;
use serde::{Deserialize, Serialize};
use specify_error::{Error, Result, is_kebab};
use specify_model::discovery::Discovery;

use super::model::{
    Entry, Plan, Severity, SliceAuthorityOverride, SliceSourceBinding, Status, TargetRef,
};
use super::validate::entry_dependency_graph;
use crate::adapter::TargetAdapter;
use crate::config::ProjectConfig;
use crate::init::adapter_name_from_value;
use crate::journal::ReconcileScope;
use crate::registry::Registry;

/// Wire version pinned by `schemas/discovery/proposal.schema.json`
/// (`const: 1` on both envelope kinds).
const PROPOSAL_VERSION: u32 = 1;

/// Closed `kind` discriminator for the reconciliation envelope.
///
/// Serialises to the literal `"request"` / `"response"` the schema's
/// `const` constraints require. [`ProposalRequest`] always carries
/// [`ProposalKind::Request`]; [`ProposalResponse`] always carries
/// [`ProposalKind::Response`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProposalKind {
    /// `kind: request` — the lead catalog plus project topology the CLI
    /// emits for the agent to group.
    Request,
    /// `kind: response` — the agent's `slices[]` grouping the CLI reads
    /// back.
    Response,
}

/// `kind: request` envelope — the lead-centric catalog the agent groups.
///
/// Emitted by `specrun plan propose --dry-run --format json`: a flat
/// `leads[]` catalog read 1:1 from `discovery.md`, plus the `projects[]`
/// topology the agent binds slices to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ProposalRequest {
    /// Wire version; always `1` per the schema `const`.
    pub version: u32,
    /// Discriminator; always [`ProposalKind::Request`].
    pub kind: ProposalKind,
    /// Project topology — always at least one entry (schema
    /// `minItems: 1`).
    pub projects: Vec<ProjectRef>,
    /// Flat lead catalog: one row per raw `(source-key, lead-id)` lead.
    pub leads: Vec<LeadCatalogEntry>,
}

/// One project the agent may bind a response slice to.
///
/// For a platform hub this mirrors a `registry.yaml#/projects[]` entry;
/// for a single regular project the CLI synthesises one entry from
/// `project.yaml` (name + resolved target adapter + domain).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ProjectRef {
    /// Project name — the value the kernel writes to
    /// `plan.yaml.slices[].project`.
    pub name: String,
    /// The project's target adapter in `name@vN` form (e.g.
    /// `omnia@v1`). Written to `plan.yaml.slices[].target` when a slice
    /// binds to this project.
    pub target: String,
    /// Single-sentence domain characterisation used by the agent when
    /// more than one project shares a target. Absent stays off the wire.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// One row in the request's flat lead catalog.
///
/// Identity is the `(source-key, lead-id)` pair; `lead-id` repeats
/// across rows when multiple sources surface the same slug. Mirrors a
/// single `discovery.md` [`specify_model::discovery::Lead`]
/// (RFC-29 D2; DECISIONS.md §"Lead reconciliation (D2)").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct LeadCatalogEntry {
    /// Plan source binding key matching `plan.yaml.sources.<key>`.
    pub source_key: String,
    /// Discovery lead id surfaced by this source binding.
    pub lead_id: String,
    /// Content-bearing per-source summary — the primary signal for
    /// agent cross-source grouping. SHOULD name the operation/surface
    /// and its salient constraint so a same-slug lead from another
    /// source can be matched or distinguished on content.
    pub summary: String,
    /// Optional alias hints from `discovery.md`. Empty list stays off
    /// the wire and is equivalent to absent.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
}

/// `kind: response` envelope — the agent's slice grouping.
///
/// Consumed by `specrun plan propose --from`. The DTO is shape-only; the
/// partition, fan-out, project-binding, and name-derivation invariants
/// are enforced by the projection kernel (a later chunk), not by serde.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ProposalResponse {
    /// Wire version; always `1` per the schema `const`.
    pub version: u32,
    /// Discriminator; always [`ProposalKind::Response`].
    pub kind: ProposalKind,
    /// The agent's slices, in response order — the kernel writes
    /// `plan.yaml.slices[]` in this order.
    pub slices: Vec<ResponseSlice>,
}

/// One `slices[]` row in a [`ProposalResponse`]: a `(scope, project)`
/// pair carrying its matched `sources[]` inline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ResponseSlice {
    /// Optional explicit plan slice name. When absent the kernel derives
    /// a name from `scope` (or `scope`-plus-`project` on fan-out).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Logical id of the reconciled unit of work — the slice-name basis
    /// and the fan-out grouping key. Propose-time only; never written to
    /// `plan.yaml`.
    pub scope: String,
    /// Matched catalog rows, each referenced by `{ source-key, lead-id }`
    /// (at most one per source). Slices sharing a `scope` carry an
    /// identical set.
    pub sources: Vec<ResponseMember>,
    /// Optional scope-level cross-source-match rationale. Attached on any
    /// one slice in a fan-out group.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rationale: Option<String>,
    /// Derived slice names this row depends on (not scope ids). Empty
    /// stays off the wire.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
    /// Project this slice binds to, chosen from the request's
    /// `projects[]`. Optional only when exactly one project exists, in
    /// which case the kernel auto-binds it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
}

/// One matched catalog row referenced by a [`ResponseSlice`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ResponseMember {
    /// Plan source binding key; must match a request catalog row.
    pub source_key: String,
    /// Discovery lead id; with `source-key`, must match a request
    /// catalog row.
    pub lead_id: String,
}

/// Set of `(source-key, lead-id)` identities surveyed in `discovery.md`.
///
/// The membership oracle the response-validation kernel (a later chunk)
/// checks every agent-supplied `{ source-key, lead-id }` against to
/// reject orphan bindings and to prove the scope partition is total.
/// Identities are deduplicated — a well-formed `discovery.md` carries a
/// unique `(source-key, lead-id)` per lead (the per-source single
/// namespace is enforced by `Discovery::check_alias_collisions`), so
/// [`LeadCatalog::len`] equals the surveyed lead count.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LeadCatalog {
    identities: BTreeSet<(String, String)>,
}

impl LeadCatalog {
    /// `true` when the `(source_key, lead_id)` identity was surveyed.
    #[must_use]
    pub fn contains(&self, source_key: &str, lead_id: &str) -> bool {
        self.identities.contains(&(source_key.to_owned(), lead_id.to_owned()))
    }

    /// Number of distinct surveyed identities.
    #[must_use]
    pub fn len(&self) -> usize {
        self.identities.len()
    }

    /// `true` when no lead was surveyed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.identities.is_empty()
    }
}

/// Build the `(source-key, lead-id)` identity set from a surveyed
/// `discovery.md`.
///
/// Shared with the response-validation kernel: `propose --from`
/// re-reads `discovery.md`, calls this to rebuild the catalog, then
/// checks every response `(source-key, lead-id)` against it. Duplicate
/// identities collapse into one set entry (see [`LeadCatalog`]).
#[must_use]
pub fn build_catalog(discovery: &Discovery) -> LeadCatalog {
    LeadCatalog {
        identities: discovery
            .leads()
            .iter()
            .map(|lead| (lead.source_key.clone(), lead.lead_id.clone()))
            .collect(),
    }
}

/// Assemble the `kind: request` envelope from a surveyed `discovery.md`
/// and an already-resolved project topology.
///
/// `leads[]` is one [`LeadCatalogEntry`] per `discovery.leads()` row,
/// carrying `source-key`, `lead-id`, `summary`, and any alias hints.
/// `projects` (produced by [`resolve_topology`]) is embedded verbatim.
///
/// # Errors
///
/// Returns [`Error::Validation`] (`plan-reconcile-empty-catalog`, exit
/// 2) when `discovery.md` carries no leads — `propose --dry-run` has
/// nothing to reconcile.
pub fn build_request(discovery: &Discovery, projects: &[ProjectRef]) -> Result<ProposalRequest> {
    let leads: Vec<LeadCatalogEntry> = discovery
        .leads()
        .iter()
        .map(|lead| LeadCatalogEntry {
            source_key: lead.source_key.clone(),
            lead_id: lead.lead_id.clone(),
            summary: lead.summary.clone(),
            aliases: lead.aliases.names.clone(),
        })
        .collect();

    if leads.is_empty() {
        return Err(Error::validation_failed(
            "plan-reconcile-empty-catalog",
            "propose --dry-run requires at least one surveyed lead",
            "discovery.md carries no leads under `## Lead inventory`",
        ));
    }

    Ok(ProposalRequest {
        version: PROPOSAL_VERSION,
        kind: ProposalKind::Request,
        projects: projects.to_vec(),
        leads,
    })
}

/// Normalise persisted project configuration into the request's
/// `projects[]` topology.
///
/// Two branches, keyed on the platform-hub discriminator
/// ([`ProjectConfig::hub`]):
///
/// - **Hub** — one [`ProjectRef`] per `registry.yaml#/projects[]` entry:
///   `name` and `description` from the registry, `target` from the
///   registry `adapter` field (already in `name@vN` form). Mirrors the
///   registry verbatim; the request schema's `minItems: 1` is the
///   downstream gate when a hub declares no projects.
/// - **Single regular project** — one synthesised [`ProjectRef`]:
///   `name` from `project.yaml.name`, `description` from
///   `project.yaml.domain`, and `target` formed by resolving
///   `project.yaml.adapter` through [`TargetAdapter::resolve`] and
///   joining the resolved adapter's canonical `name` + `version` into
///   `name@vN`.
///
/// The hub branch is filesystem-free; the regular branch resolves the
/// target adapter under `project_dir` (the only filesystem access in
/// this module).
///
/// # Errors
///
/// - [`Error::Validation`] (`plan-propose-project-adapter-missing`) when
///   a non-hub `project.yaml` omits `adapter` — a corrupt project that
///   `specrun init` never produces.
/// - Any error from [`TargetAdapter::resolve`] (`adapter-not-found`,
///   `adapter-schema-violation`, …) when the regular project's adapter
///   cannot be resolved.
pub fn resolve_topology(
    config: &ProjectConfig, registry: Option<&Registry>, project_dir: &Path,
) -> Result<Vec<ProjectRef>> {
    if config.hub {
        Ok(hub_topology(registry))
    } else {
        regular_topology(config, project_dir).map(|project| vec![project])
    }
}

/// Map every `registry.yaml#/projects[]` entry into a [`ProjectRef`].
fn hub_topology(registry: Option<&Registry>) -> Vec<ProjectRef> {
    registry
        .map(|registry| registry.projects.as_slice())
        .unwrap_or_default()
        .iter()
        .map(|project| ProjectRef {
            name: project.name.clone(),
            target: project.adapter.clone(),
            description: project.description.clone(),
        })
        .collect()
}

/// Synthesise the sole [`ProjectRef`] for a single regular project.
fn regular_topology(config: &ProjectConfig, project_dir: &Path) -> Result<ProjectRef> {
    let adapter_value = config.adapter.as_deref().ok_or_else(|| {
        Error::validation_failed(
            "plan-propose-project-adapter-missing",
            "a regular project.yaml declares an adapter",
            "non-hub project.yaml omits the `adapter` field",
        )
    })?;
    let resolved = TargetAdapter::resolve(adapter_name_from_value(adapter_value), project_dir)?;
    let target = format!("{}@v{}", resolved.manifest.name, resolved.manifest.version);
    Ok(ProjectRef {
        name: config.name.clone(),
        target,
        description: config.domain.clone(),
    })
}

/// Outcome of a successful [`Plan::propose_from`] projection.
///
/// The reconciliation kernel returns it so the CLI handler (the
/// `propose --from` command) can emit the two D2 journal events
/// without re-deriving anything: [`ProposeOutcome::scopes`] feeds the
/// deduped `plan.reconcile.agent` payload, and
/// [`ProposeOutcome::slice_names`] (plus its length) feeds
/// `plan.reconcile.completed`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposeOutcome {
    /// Derived slice names, in the agent's `slices[]` response order —
    /// the same order the kernel wrote `plan.yaml.slices[]`.
    pub slice_names: Vec<String>,
    /// Reconciled scopes deduped by `scope` id in first-appearance
    /// order, each carrying the scope's first non-empty cross-source
    /// `rationale` (a fan-out scope contributes one entry).
    pub scopes: Vec<ReconcileScope>,
}

impl Plan {
    /// Project a validated agent reconciliation response onto
    /// `plan.yaml.slices[]` (RFC-29 D2 projection kernel;
    /// DECISIONS.md §"Lead reconciliation (D2)").
    ///
    /// `response` is assumed to have already passed JSON-Schema
    /// validation (`validate_proposal_json`) at the CLI boundary, so this
    /// method enforces only the *semantic* invariants the schema cannot
    /// express. The checks fire in this order, returning the first
    /// violation: replaceable gate (`plan-reconcile-plan-not-replaceable`); lead-orphan (`plan-reconcile-lead-orphan`); per-slice same-source fusion (`plan-reconcile-slice-source-collision`); fan-out source consistency (`plan-reconcile-fanout-source-mismatch`); total global partition (`plan-reconcile-partition`); project auto-bind / orphan (`plan-reconcile-project-binding-required`, `plan-reconcile-project-orphan`); unique `(scope, project)` pairs (`plan-reconcile-slice-duplicate`); slice-name derivation + collision (`plan-reconcile-slice-name-collision`); `depends-on` cycle (`plan-reconcile-depends-on-cycle`); and finally a backstop [`Plan::validate`] over the projected entries that rolls the plan back on any blocking finding.
    ///
    /// On success `self.entries` is the projected slice set in response
    /// order and the returned [`ProposeOutcome`] carries the derived
    /// names and deduped scopes for the caller's journal events.
    ///
    /// # Errors
    ///
    /// Returns an [`Error::Validation`] (exit 2) carrying the first
    /// invariant code listed above that the response violates.
    pub fn propose_from(
        &mut self, response: ProposalResponse, discovery: &Discovery, topology: &[ProjectRef],
    ) -> Result<ProposeOutcome> {
        if !self.is_replaceable() {
            return Err(Error::validation_failed(
                "plan-reconcile-plan-not-replaceable",
                "propose --from requires a replaceable plan",
                "lifecycle is approved or any entry is in-progress or done",
            ));
        }

        let catalog = build_catalog(discovery);
        let slices = response.slices.as_slice();

        check_lead_orphans(slices, &catalog)?;
        let scope_sets = group_scopes(slices, &slice_source_sets(slices)?)?;
        check_partition(&scope_sets, &catalog)?;
        let bound = bind_projects(slices, topology)?;
        check_slice_duplicates(slices, &bound)?;
        let targets = derive_targets(&bound)?;
        let names = derive_names(slices, &bound)?;
        check_name_collisions(&names)?;

        let scopes = dedup_scopes(&response);
        let new_entries = build_entries(response.slices, &names, &bound, &targets);

        if has_dependency_cycle(&new_entries) {
            return Err(Error::validation_failed(
                "plan-reconcile-depends-on-cycle",
                "the depends-on graph must be acyclic",
                "the projected slices form a depends-on cycle",
            ));
        }

        // Bulk replace, run the backstop validate, and roll back on any
        // blocking finding (e.g. unknown depends-on names).
        let previous = std::mem::replace(&mut self.entries, new_entries);
        if let Some(finding) =
            self.validate(None, None).into_iter().find(|f| f.level == Severity::Error)
        {
            self.entries = previous;
            return Err(Error::validation_failed(finding.code, String::new(), finding.message));
        }

        Ok(ProposeOutcome {
            slice_names: names,
            scopes,
        })
    }
}

/// A `(source-key, lead-id)` catalog identity.
type LeadPair = (String, String);

/// The order-insensitive `sources[]` membership of one scope.
type ScopeMembership = BTreeSet<LeadPair>;

/// Lead-orphan: every cited `(source-key, lead-id)` must name a current
/// catalog row (`plan-reconcile-lead-orphan`).
fn check_lead_orphans(slices: &[ResponseSlice], catalog: &LeadCatalog) -> Result<()> {
    for slice in slices {
        for member in &slice.sources {
            if !catalog.contains(&member.source_key, &member.lead_id) {
                return Err(Error::validation_failed(
                    "plan-reconcile-lead-orphan",
                    "every cited source binding must name a surveyed lead",
                    format!(
                        "({}, {}) is not in the discovery.md lead catalog",
                        member.source_key, member.lead_id
                    ),
                ));
            }
        }
    }
    Ok(())
}

/// Per-slice source set, enforcing at-most-one-lead-per-source
/// (`plan-reconcile-slice-source-collision`).
fn slice_source_sets(slices: &[ResponseSlice]) -> Result<Vec<ScopeMembership>> {
    let mut out = Vec::with_capacity(slices.len());
    for slice in slices {
        let mut set = BTreeSet::new();
        let mut keys: HashSet<&str> = HashSet::new();
        for member in &slice.sources {
            if !keys.insert(member.source_key.as_str()) {
                return Err(Error::validation_failed(
                    "plan-reconcile-slice-source-collision",
                    "a scope names at most one lead per source",
                    format!(
                        "scope '{}' names source '{}' more than once",
                        slice.scope, member.source_key
                    ),
                ));
            }
            set.insert((member.source_key.clone(), member.lead_id.clone()));
        }
        out.push(set);
    }
    Ok(out)
}

/// Collapse slices into per-scope membership in first-appearance order,
/// enforcing fan-out consistency (`plan-reconcile-fanout-source-mismatch`):
/// slices sharing a `scope` carry an identical `sources[]` set.
fn group_scopes(
    slices: &[ResponseSlice], sets: &[ScopeMembership],
) -> Result<Vec<(String, ScopeMembership)>> {
    let mut scope_sets: Vec<(String, ScopeMembership)> = Vec::new();
    for (idx, slice) in slices.iter().enumerate() {
        let set = &sets[idx];
        match scope_sets.iter().find(|(scope, _)| scope == &slice.scope) {
            Some((_, existing)) if existing != set => {
                return Err(Error::validation_failed(
                    "plan-reconcile-fanout-source-mismatch",
                    "slices sharing a scope must carry identical sources",
                    format!("scope '{}' has fan-out rows with differing sources", slice.scope),
                ));
            }
            Some(_) => {}
            None => scope_sets.push((slice.scope.clone(), set.clone())),
        }
    }
    Ok(scope_sets)
}

/// Total global partition: every catalog lead lands in exactly one scope —
/// none missing, none in two scopes (`plan-reconcile-partition`).
fn check_partition(scope_sets: &[(String, ScopeMembership)], catalog: &LeadCatalog) -> Result<()> {
    let mut owner: HashMap<LeadPair, String> = HashMap::new();
    for (scope, set) in scope_sets {
        for pair in set {
            if let Some(prev) = owner.insert(pair.clone(), scope.clone()) {
                return Err(Error::validation_failed(
                    "plan-reconcile-partition",
                    "each surveyed lead belongs to exactly one scope",
                    format!(
                        "lead ({}, {}) appears in scopes '{prev}' and '{scope}'",
                        pair.0, pair.1
                    ),
                ));
            }
        }
    }
    for pair in &catalog.identities {
        if !owner.contains_key(pair) {
            return Err(Error::validation_failed(
                "plan-reconcile-partition",
                "each surveyed lead belongs to exactly one scope",
                format!("lead ({}, {}) is unaccounted for by any scope", pair.0, pair.1),
            ));
        }
    }
    Ok(())
}

/// Bind each slice to a project: an explicit `project` must exist in the
/// topology (`plan-reconcile-project-orphan`); an omitted `project`
/// auto-binds the sole project or fails when several exist
/// (`plan-reconcile-project-binding-required`).
fn bind_projects<'a>(
    slices: &[ResponseSlice], topology: &'a [ProjectRef],
) -> Result<Vec<&'a ProjectRef>> {
    let mut bound = Vec::with_capacity(slices.len());
    for slice in slices {
        let project = match &slice.project {
            Some(name) => topology.iter().find(|p| &p.name == name).ok_or_else(|| {
                Error::validation_failed(
                    "plan-reconcile-project-orphan",
                    "a bound project must exist in the request topology",
                    format!("slice scope '{}' binds unknown project '{name}'", slice.scope),
                )
            })?,
            None if topology.len() == 1 => &topology[0],
            None => {
                return Err(Error::validation_failed(
                    "plan-reconcile-project-binding-required",
                    "a slice may omit project only when exactly one project exists",
                    format!(
                        "scope '{}' omits project but {} projects are available",
                        slice.scope,
                        topology.len()
                    ),
                ));
            }
        };
        bound.push(project);
    }
    Ok(bound)
}

/// Unique `(scope, project)` pairs (`plan-reconcile-slice-duplicate`).
fn check_slice_duplicates(slices: &[ResponseSlice], bound: &[&ProjectRef]) -> Result<()> {
    let mut seen: HashSet<(&str, &str)> = HashSet::new();
    for (idx, slice) in slices.iter().enumerate() {
        if !seen.insert((slice.scope.as_str(), bound[idx].name.as_str())) {
            return Err(Error::validation_failed(
                "plan-reconcile-slice-duplicate",
                "each (scope, project) pair maps to one slice",
                format!("scope '{}' binds project '{}' twice", slice.scope, bound[idx].name),
            ));
        }
    }
    Ok(())
}

/// Derive each slice's `target` from its bound project. Topology targets
/// are pre-validated, so a parse failure is an internal inconsistency
/// surfaced as `plan-target-malformed`.
fn derive_targets(bound: &[&ProjectRef]) -> Result<Vec<TargetRef>> {
    bound
        .iter()
        .map(|project| {
            TargetRef::parse(&project.target).map_err(|err| {
                Error::validation_failed(
                    "plan-target-malformed",
                    "a project target must parse as name@vN",
                    err.to_string(),
                )
            })
        })
        .collect()
}

/// Derive each slice name (RFC-29 D2 slice-name derivation): an explicit name, else
/// `scope` for a 1:1 scope, else `<scope>-<project>` across a fan-out
/// group. Every derived or explicit name must be kebab-case.
fn derive_names(slices: &[ResponseSlice], bound: &[&ProjectRef]) -> Result<Vec<String>> {
    let mut scope_count: HashMap<&str, usize> = HashMap::new();
    for slice in slices {
        *scope_count.entry(slice.scope.as_str()).or_default() += 1;
    }
    let mut names = Vec::with_capacity(slices.len());
    for (idx, slice) in slices.iter().enumerate() {
        let name = match &slice.name {
            Some(explicit) => explicit.clone(),
            None if scope_count[slice.scope.as_str()] == 1 => slice.scope.clone(),
            None => format!("{}-{}", slice.scope, bound[idx].name),
        };
        if !is_kebab(&name) {
            return Err(Error::validation_failed(
                "plan-reconcile-slice-name-invalid",
                "a derived or explicit slice name must be kebab-case",
                format!("slice name '{name}' is not kebab-case"),
            ));
        }
        names.push(name);
    }
    Ok(names)
}

/// Reject clashing final slice names. Derived names are unique by
/// construction, so a clash necessarily involves an agent-supplied
/// explicit name (`plan-reconcile-slice-name-collision`).
fn check_name_collisions(names: &[String]) -> Result<()> {
    let mut seen: HashSet<&str> = HashSet::new();
    for name in names {
        if !seen.insert(name.as_str()) {
            return Err(Error::validation_failed(
                "plan-reconcile-slice-name-collision",
                "explicit slice names must be unique",
                format!("two slices resolve to the name '{name}'"),
            ));
        }
    }
    Ok(())
}

/// Project the validated response into `plan.yaml.slices[]` entries in
/// response order, consuming the response's slices.
fn build_entries(
    slices: Vec<ResponseSlice>, names: &[String], bound: &[&ProjectRef], targets: &[TargetRef],
) -> Vec<Entry> {
    slices
        .into_iter()
        .enumerate()
        .map(|(idx, slice)| Entry {
            name: names[idx].clone(),
            project: Some(bound[idx].name.clone()),
            target: Some(targets[idx].clone()),
            status: Status::Pending,
            depends_on: slice.depends_on,
            sources: slice
                .sources
                .into_iter()
                .map(|m| SliceSourceBinding::structured(m.source_key, m.lead_id))
                .collect(),
            context: Vec::new(),
            description: None,
            divergence: None,
            authority_override: SliceAuthorityOverride::default(),
        })
        .collect()
}

/// `true` when the projected entries' `depends-on` graph contains a
/// cycle (including a self-loop), reusing the same Tarjan-SCC detection
/// as the `cycle-in-depends-on` doctor diagnostic.
fn has_dependency_cycle(entries: &[Entry]) -> bool {
    let graph = entry_dependency_graph(entries);
    tarjan_scc(&graph)
        .into_iter()
        .any(|scc| scc.len() > 1 || (scc.len() == 1 && graph.find_edge(scc[0], scc[0]).is_some()))
}

/// Dedupe a response's scopes by `scope` id in first-appearance order,
/// carrying each scope's first non-`None` cross-source `rationale`.
fn dedup_scopes(response: &ProposalResponse) -> Vec<ReconcileScope> {
    let mut order: Vec<&str> = Vec::new();
    let mut seen: HashSet<&str> = HashSet::new();
    for slice in &response.slices {
        if seen.insert(slice.scope.as_str()) {
            order.push(slice.scope.as_str());
        }
    }
    order
        .into_iter()
        .map(|scope| ReconcileScope {
            scope: scope.to_string(),
            rationale: response
                .slices
                .iter()
                .filter(|s| s.scope == scope)
                .find_map(|s| s.rationale.clone()),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use specify_error::Error;

    use super::super::model::{Lifecycle, SourceBinding};
    use super::*;
    use crate::registry::{Registry, RegistryProject};
    use crate::schema::validate_proposal_json;

    fn discovery(body: &str) -> Discovery {
        Discovery::parse(body).expect("discovery parses")
    }

    fn project(name: &str, target: &str, description: &str) -> ProjectRef {
        ProjectRef {
            name: name.to_string(),
            target: target.to_string(),
            description: Some(description.to_string()),
        }
    }

    #[test]
    fn build_request_n1_validates_as_request() {
        let doc = discovery(
            "## Lead inventory\n\n\
             ### intent:fix-typo\n\n\
             - lead-id: fix-typo\n\
             - source-key: intent\n\
             - summary: fix typo in user.rs\n",
        );
        let topology =
            vec![project("my-app", "omnia@v1", "Single Omnia service for this repository.")];

        let request = build_request(&doc, &topology).expect("request builds");
        assert_eq!(request.version, PROPOSAL_VERSION);
        assert_eq!(request.kind, ProposalKind::Request);
        assert_eq!(request.projects, topology);
        assert_eq!(request.leads.len(), 1);
        assert_eq!(request.leads[0].source_key, "intent");
        assert_eq!(request.leads[0].lead_id, "fix-typo");
        assert!(request.leads[0].aliases.is_empty());

        let json = serde_json::to_string(&request).expect("serialise request");
        assert!(json.contains(r#""kind":"request""#), "kind must render as request: {json}");
        validate_proposal_json(&json).expect("N=1 request validates against the schema");
    }

    #[test]
    fn build_request_hub_validates_as_request() {
        let doc = discovery(
            "## Lead inventory\n\n\
             ### docs:identity-api\n\n\
             - lead-id: identity-api\n\
             - source-key: docs\n\
             - aliases: [auth-api]\n\
             - summary: Identity API contract.\n\n\
             ### legacy:identity-api\n\n\
             - lead-id: identity-api\n\
             - source-key: legacy\n\
             - summary: Legacy identity endpoints.\n\n\
             ### docs:password-reset\n\n\
             - lead-id: password-reset\n\
             - source-key: docs\n\
             - summary: Users can request a password reset email.\n",
        );
        let topology = vec![
            project("identity-contracts", "contracts@v1", "Versioned API contracts crate."),
            project("identity-service", "omnia@v1", "Omnia identity service."),
        ];

        let request = build_request(&doc, &topology).expect("request builds");
        assert_eq!(request.leads.len(), 3);
        assert_eq!(request.leads[0].aliases, vec!["auth-api"]);

        let json = serde_json::to_string(&request).expect("serialise request");
        validate_proposal_json(&json).expect("hub request validates against the schema");
    }

    #[test]
    fn build_request_empty_catalog_errors() {
        let doc = discovery("# Discovery\n\nNo leads surveyed yet.\n\n## Lead inventory\n");
        let topology = vec![project("my-app", "omnia@v1", "Single service.")];

        match build_request(&doc, &topology) {
            Err(Error::Validation { code, .. }) => {
                assert_eq!(code, "plan-reconcile-empty-catalog");
            }
            other => panic!("expected empty-catalog validation error, got {other:?}"),
        }
    }

    #[test]
    fn build_catalog_membership_and_size() {
        let doc = discovery(
            "## Lead inventory\n\n\
             ### docs:identity-api\n\n\
             - lead-id: identity-api\n\
             - source-key: docs\n\
             - summary: Identity API.\n\n\
             ### legacy:identity-api\n\n\
             - lead-id: identity-api\n\
             - source-key: legacy\n\
             - summary: Legacy identity.\n",
        );
        let catalog = build_catalog(&doc);

        assert_eq!(catalog.len(), 2);
        assert!(!catalog.is_empty());
        assert!(catalog.contains("docs", "identity-api"));
        assert!(catalog.contains("legacy", "identity-api"));
        // Same slug under the wrong source is not in the catalog.
        assert!(!catalog.contains("docs", "password-reset"));
    }

    #[test]
    fn response_round_trips_rfc_multi_source_example() {
        // Multi-source fan-out response (the proposal-schema envelope example).
        let yaml = "\
version: 1
kind: response
slices:
  - name: identity-contracts
    scope: identity-api
    sources:
      - { source-key: docs, lead-id: identity-api }
      - { source-key: legacy, lead-id: identity-api }
    project: identity-contracts
    rationale: \"identity API surface matched by shared slug across docs + legacy\"
  - name: identity-service
    scope: identity-api
    sources:
      - { source-key: docs, lead-id: identity-api }
      - { source-key: legacy, lead-id: identity-api }
    project: identity-service
    depends-on: [identity-contracts]
  - name: password-reset
    scope: password-reset
    sources:
      - { source-key: docs, lead-id: password-reset }
      - { source-key: legacy, lead-id: reset-password }
    project: identity-service
    rationale: \"password-reset (docs) and reset-password (legacy) are the same flow by summary judgment\"
";
        let response: ProposalResponse =
            serde_saphyr::from_str(yaml).expect("response deserialises");

        assert_eq!(response.version, PROPOSAL_VERSION);
        assert_eq!(response.kind, ProposalKind::Response);
        assert_eq!(response.slices.len(), 3);

        let contracts = &response.slices[0];
        assert_eq!(contracts.name.as_deref(), Some("identity-contracts"));
        assert_eq!(contracts.scope, "identity-api");
        assert_eq!(contracts.project.as_deref(), Some("identity-contracts"));
        assert_eq!(contracts.sources.len(), 2);
        assert_eq!(contracts.sources[0].source_key, "docs");
        assert_eq!(contracts.sources[0].lead_id, "identity-api");
        assert!(contracts.depends_on.is_empty());

        let service = &response.slices[1];
        assert_eq!(service.depends_on, vec!["identity-contracts"]);
        assert!(service.rationale.is_none());

        let reset = &response.slices[2];
        assert_eq!(reset.scope, "password-reset");
        assert_eq!(reset.sources[1].source_key, "legacy");
        assert_eq!(reset.sources[1].lead_id, "reset-password");

        // The DTO re-serialises into a schema-valid response, locking the
        // shape the projection kernel will consume.
        let json = serde_json::to_string(&response).expect("serialise response");
        validate_proposal_json(&json).expect("round-tripped response validates");
    }

    #[test]
    fn resolve_topology_hub_maps_registry_projects() {
        let config = ProjectConfig {
            name: "platform".to_string(),
            domain: None,
            adapter: None,
            specify_version: None,
            rules: std::collections::BTreeMap::new(),
            tools: Vec::new(),
            hub: true,
        };
        let registry = Registry {
            version: 1,
            projects: vec![
                RegistryProject {
                    name: "identity-contracts".to_string(),
                    url: "./contracts".to_string(),
                    adapter: "contracts@v1".to_string(),
                    description: Some("Contracts crate.".to_string()),
                    contracts: None,
                },
                RegistryProject {
                    name: "identity-service".to_string(),
                    url: "git@github.com:org/identity.git".to_string(),
                    adapter: "omnia@v1".to_string(),
                    description: None,
                    contracts: None,
                },
            ],
        };

        let topology = resolve_topology(&config, Some(&registry), Path::new("/unused"))
            .expect("hub topology resolves");
        assert_eq!(
            topology,
            vec![
                ProjectRef {
                    name: "identity-contracts".to_string(),
                    target: "contracts@v1".to_string(),
                    description: Some("Contracts crate.".to_string()),
                },
                ProjectRef {
                    name: "identity-service".to_string(),
                    target: "omnia@v1".to_string(),
                    description: None,
                },
            ]
        );
    }

    #[test]
    fn resolve_topology_regular_missing_adapter_errors() {
        let config = ProjectConfig {
            name: "demo".to_string(),
            domain: None,
            adapter: None,
            specify_version: None,
            rules: std::collections::BTreeMap::new(),
            tools: Vec::new(),
            hub: false,
        };
        match resolve_topology(&config, None, Path::new("/unused")) {
            Err(Error::Validation { code, .. }) => {
                assert_eq!(code, "plan-propose-project-adapter-missing");
            }
            other => panic!("expected adapter-missing validation error, got {other:?}"),
        }
    }

    // --- propose_from projection kernel ---------------------------------

    fn member(source_key: &str, lead_id: &str) -> ResponseMember {
        ResponseMember {
            source_key: source_key.to_string(),
            lead_id: lead_id.to_string(),
        }
    }

    fn slice(scope: &str, sources: Vec<ResponseMember>) -> ResponseSlice {
        ResponseSlice {
            name: None,
            scope: scope.to_string(),
            sources,
            rationale: None,
            depends_on: Vec::new(),
            project: None,
        }
    }

    fn response(slices: Vec<ResponseSlice>) -> ProposalResponse {
        ProposalResponse {
            version: PROPOSAL_VERSION,
            kind: ProposalKind::Response,
            slices,
        }
    }

    fn discovery_with(leads: &[(&str, &str)]) -> Discovery {
        let body: String = std::iter::once("## Lead inventory\n\n".to_string())
            .chain(leads.iter().map(|(source_key, lead_id)| {
                format!(
                    "### {source_key}:{lead_id}\n\n\
                     - lead-id: {lead_id}\n\
                     - source-key: {source_key}\n\
                     - summary: {lead_id} summary\n\n",
                )
            }))
            .collect();
        discovery(&body)
    }

    fn plan_with_sources(lifecycle: Lifecycle, keys: &[&str]) -> Plan {
        Plan {
            name: "p".to_string(),
            lifecycle,
            sources: keys
                .iter()
                .map(|k| ((*k).to_string(), SourceBinding::value("intent", "brief")))
                .collect(),
            entries: Vec::new(),
        }
    }

    fn assert_code(result: Result<ProposeOutcome>, expected: &str) {
        match result {
            Err(Error::Validation { code, .. }) => assert_eq!(code, expected),
            other => panic!("expected {expected} validation error, got {other:?}"),
        }
    }

    #[test]
    fn propose_rejects_non_replaceable_plan() {
        let mut plan = plan_with_sources(Lifecycle::Approved, &["intent"]);
        let doc = discovery_with(&[("intent", "fix-typo")]);
        let topo = vec![project("my-app", "omnia@v1", "Single service.")];
        let resp = response(vec![slice("fix-typo", vec![member("intent", "fix-typo")])]);
        assert_code(plan.propose_from(resp, &doc, &topo), "plan-reconcile-plan-not-replaceable");
    }

    #[test]
    fn propose_rejects_lead_orphan() {
        let mut plan = plan_with_sources(Lifecycle::Pending, &["docs"]);
        let doc = discovery_with(&[("docs", "real")]);
        let topo = vec![project("p", "omnia@v1", "svc")];
        let resp = response(vec![slice("s", vec![member("docs", "ghost")])]);
        assert_code(plan.propose_from(resp, &doc, &topo), "plan-reconcile-lead-orphan");
    }

    #[test]
    fn propose_rejects_slice_source_collision() {
        let mut plan = plan_with_sources(Lifecycle::Pending, &["docs"]);
        let doc = discovery_with(&[("docs", "a"), ("docs", "b")]);
        let topo = vec![project("p", "omnia@v1", "svc")];
        let resp = response(vec![slice("s", vec![member("docs", "a"), member("docs", "b")])]);
        assert_code(plan.propose_from(resp, &doc, &topo), "plan-reconcile-slice-source-collision");
    }

    #[test]
    fn propose_rejects_fanout_source_mismatch() {
        let mut plan = plan_with_sources(Lifecycle::Pending, &["docs"]);
        let doc = discovery_with(&[("docs", "a"), ("docs", "b")]);
        let topo = vec![project("p", "omnia@v1", "svc")];
        let resp = response(vec![
            slice("s", vec![member("docs", "a")]),
            slice("s", vec![member("docs", "b")]),
        ]);
        assert_code(plan.propose_from(resp, &doc, &topo), "plan-reconcile-fanout-source-mismatch");
    }

    #[test]
    fn propose_rejects_partition_gap() {
        let mut plan = plan_with_sources(Lifecycle::Pending, &["docs"]);
        let doc = discovery_with(&[("docs", "a"), ("docs", "b")]);
        let topo = vec![project("p", "omnia@v1", "svc")];
        // Catalog carries two leads; the response covers only one.
        let resp = response(vec![slice("s", vec![member("docs", "a")])]);
        assert_code(plan.propose_from(resp, &doc, &topo), "plan-reconcile-partition");
    }

    #[test]
    fn propose_rejects_project_binding_required() {
        let mut plan = plan_with_sources(Lifecycle::Pending, &["docs"]);
        let doc = discovery_with(&[("docs", "a")]);
        let topo =
            vec![project("p1", "omnia@v1", "first"), project("p2", "contracts@v1", "second")];
        // Two projects offered, slice omits `project`.
        let resp = response(vec![slice("s", vec![member("docs", "a")])]);
        assert_code(
            plan.propose_from(resp, &doc, &topo),
            "plan-reconcile-project-binding-required",
        );
    }

    #[test]
    fn propose_rejects_project_orphan() {
        let mut plan = plan_with_sources(Lifecycle::Pending, &["docs"]);
        let doc = discovery_with(&[("docs", "a")]);
        let topo = vec![project("p", "omnia@v1", "svc")];
        let mut s = slice("s", vec![member("docs", "a")]);
        s.project = Some("ghost".to_string());
        assert_code(
            plan.propose_from(response(vec![s]), &doc, &topo),
            "plan-reconcile-project-orphan",
        );
    }

    #[test]
    fn propose_rejects_slice_duplicate() {
        let mut plan = plan_with_sources(Lifecycle::Pending, &["docs"]);
        let doc = discovery_with(&[("docs", "a")]);
        let topo = vec![project("p", "omnia@v1", "svc")];
        // Two slices share one (scope, project) pair with identical sources.
        let bind = || {
            let mut s = slice("s", vec![member("docs", "a")]);
            s.project = Some("p".to_string());
            s
        };
        assert_code(
            plan.propose_from(response(vec![bind(), bind()]), &doc, &topo),
            "plan-reconcile-slice-duplicate",
        );
    }

    #[test]
    fn propose_rejects_slice_name_collision() {
        let mut plan = plan_with_sources(Lifecycle::Pending, &["docs"]);
        let doc = discovery_with(&[("docs", "a"), ("docs", "b")]);
        let topo = vec![project("p", "omnia@v1", "svc")];
        let mut s1 = slice("s1", vec![member("docs", "a")]);
        s1.name = Some("dup".to_string());
        s1.project = Some("p".to_string());
        let mut s2 = slice("s2", vec![member("docs", "b")]);
        s2.name = Some("dup".to_string());
        s2.project = Some("p".to_string());
        assert_code(
            plan.propose_from(response(vec![s1, s2]), &doc, &topo),
            "plan-reconcile-slice-name-collision",
        );
    }

    #[test]
    fn propose_rejects_depends_on_cycle() {
        let mut plan = plan_with_sources(Lifecycle::Pending, &["docs"]);
        let doc = discovery_with(&[("docs", "a"), ("docs", "b")]);
        let topo = vec![project("p", "omnia@v1", "svc")];
        let mut s1 = slice("alpha-scope", vec![member("docs", "a")]);
        s1.name = Some("alpha".to_string());
        s1.project = Some("p".to_string());
        s1.depends_on = vec!["beta".to_string()];
        let mut s2 = slice("beta-scope", vec![member("docs", "b")]);
        s2.name = Some("beta".to_string());
        s2.project = Some("p".to_string());
        s2.depends_on = vec!["alpha".to_string()];
        assert_code(
            plan.propose_from(response(vec![s1, s2]), &doc, &topo),
            "plan-reconcile-depends-on-cycle",
        );
    }

    #[test]
    fn propose_n1_auto_binds_sole_project() {
        let mut plan = plan_with_sources(Lifecycle::Pending, &["intent"]);
        let doc = discovery_with(&[("intent", "fix-typo")]);
        let topo = vec![project("my-app", "omnia@v1", "Single Omnia service.")];
        // No explicit name (derives from scope) and no project (auto-bound).
        let resp = response(vec![slice("fix-typo", vec![member("intent", "fix-typo")])]);

        let out = plan.propose_from(resp, &doc, &topo).expect("N=1 projects");

        assert_eq!(out.slice_names, vec!["fix-typo"]);
        assert_eq!(out.scopes.len(), 1);
        assert_eq!(out.scopes[0].scope, "fix-typo");
        assert_eq!(out.scopes[0].rationale, None);

        assert_eq!(plan.entries.len(), 1);
        let entry = &plan.entries[0];
        assert_eq!(entry.name, "fix-typo");
        assert_eq!(entry.project.as_deref(), Some("my-app"));
        assert_eq!(entry.target.as_ref().map(ToString::to_string), Some("omnia@v1".to_string()));
        assert_eq!(entry.status, Status::Pending);
        assert!(entry.depends_on.is_empty());
        assert_eq!(entry.sources, vec![SliceSourceBinding::structured("intent", "fix-typo")]);
    }

    #[test]
    fn propose_multi_source_fan_out() {
        let doc = discovery_with(&[
            ("docs", "identity-api"),
            ("legacy", "identity-api"),
            ("docs", "password-reset"),
            ("legacy", "reset-password"),
        ]);
        let topo = vec![
            project("identity-contracts", "contracts@v1", "Versioned API contracts crate."),
            project("identity-service", "omnia@v1", "Omnia identity service."),
        ];
        let mut plan = plan_with_sources(Lifecycle::Pending, &["docs", "legacy"]);

        // Multi-source fan-out response (the proposal-schema envelope example).
        let yaml = "\
version: 1
kind: response
slices:
  - name: identity-contracts
    scope: identity-api
    sources:
      - { source-key: docs, lead-id: identity-api }
      - { source-key: legacy, lead-id: identity-api }
    project: identity-contracts
    rationale: \"identity API surface matched by shared slug across docs + legacy\"
  - name: identity-service
    scope: identity-api
    sources:
      - { source-key: docs, lead-id: identity-api }
      - { source-key: legacy, lead-id: identity-api }
    project: identity-service
    depends-on: [identity-contracts]
  - name: password-reset
    scope: password-reset
    sources:
      - { source-key: docs, lead-id: password-reset }
      - { source-key: legacy, lead-id: reset-password }
    project: identity-service
    rationale: \"password-reset (docs) and reset-password (legacy) are the same flow by summary judgment\"
";
        let resp: ProposalResponse =
            serde_saphyr::from_str(yaml).expect("multi-source response deserialises");

        let out = plan.propose_from(resp, &doc, &topo).expect("fan-out projects");

        assert_eq!(
            out.slice_names,
            vec!["identity-contracts", "identity-service", "password-reset"]
        );
        // Two distinct scopes; the fan-out scope dedupes to one entry and
        // carries its rationale.
        assert_eq!(out.scopes.len(), 2);
        assert_eq!(out.scopes[0].scope, "identity-api");
        assert_eq!(
            out.scopes[0].rationale.as_deref(),
            Some("identity API surface matched by shared slug across docs + legacy")
        );
        assert_eq!(out.scopes[1].scope, "password-reset");
        assert_eq!(
            out.scopes[1].rationale.as_deref(),
            Some(
                "password-reset (docs) and reset-password (legacy) are the same flow by summary judgment"
            )
        );

        assert_eq!(plan.entries.len(), 3);

        let names: Vec<&str> = plan.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["identity-contracts", "identity-service", "password-reset"]);

        let projects: Vec<Option<&str>> =
            plan.entries.iter().map(|e| e.project.as_deref()).collect();
        assert_eq!(
            projects,
            vec![Some("identity-contracts"), Some("identity-service"), Some("identity-service")]
        );

        let targets: Vec<Option<String>> =
            plan.entries.iter().map(|e| e.target.as_ref().map(ToString::to_string)).collect();
        assert_eq!(
            targets,
            vec![
                Some("contracts@v1".to_string()),
                Some("omnia@v1".to_string()),
                Some("omnia@v1".to_string()),
            ]
        );

        assert_eq!(
            plan.entries[0].sources,
            vec![
                SliceSourceBinding::structured("docs", "identity-api"),
                SliceSourceBinding::structured("legacy", "identity-api"),
            ]
        );
        assert_eq!(
            plan.entries[2].sources,
            vec![
                SliceSourceBinding::structured("docs", "password-reset"),
                SliceSourceBinding::structured("legacy", "reset-password"),
            ]
        );

        assert!(plan.entries[0].depends_on.is_empty());
        assert_eq!(plan.entries[1].depends_on, vec!["identity-contracts"]);
    }
}
