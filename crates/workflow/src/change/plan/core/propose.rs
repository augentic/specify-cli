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
//!   [`LeadCatalog`] of `(source, lead)` identities, the
//!   membership oracle the response-validation kernel (a later chunk)
//!   checks every agent-supplied source binding against.
//! - [`resolve_topology`] normalises persisted project configuration
//!   (a workspace root's committed `.specify/topology.lock`, or the sole
//!   project synthesised from `project.yaml`) into the envelope-local
//!   [`ProjectRef`] list (RFC-36).
//!
//! `build_request` / `build_catalog` are pure so they unit-test without
//! a temp project; the only filesystem access lives in
//! [`resolve_topology`], which reads the workspace topology cache or resolves
//! the regular project's target adapter to its canonical `name@vN` ref.

use std::collections::{BTreeSet, HashSet};
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
use crate::config::{Layout, ProjectConfig};
use crate::init::adapter_name_from_value;
use crate::registry::topology::{Decision, Surface, TopologyLock};

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
    /// Flat lead catalog: one row per raw `(source, lead)` lead.
    pub leads: Vec<LeadCatalogEntry>,
}

/// One project the agent may bind a response slice to.
///
/// For a workspace root this is projected from the committed
/// `.specify/topology.lock` (RFC-36); for a single regular project the
/// CLI synthesises one entry from `project.yaml` (name + resolved
/// target adapter + description) plus the project's own baseline
/// projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ProjectRef {
    /// Project name — the value the kernel writes to
    /// `plan.yaml.slices[].project`.
    pub name: String,
    /// The project's target adapter in `name@vN` form (e.g.
    /// `omnia@v1`). Resolved on demand by [`resolve_target`] for a slice
    /// bound to this project; it is no longer written to `plan.yaml`
    /// (a slice stores only its `project`).
    pub target: String,
    /// Single-sentence domain characterisation used by the agent when
    /// more than one project shares a target. Absent stays off the wire.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Deterministic baseline surface (RFC-36): the units this project
    /// owns and a sample of each unit's requirement titles, projected
    /// from `.specify/specs/` through `.specify/topology.lock`. The
    /// agent binds a slice on actual owned behaviour. Empty stays off
    /// the wire (greenfield routes on `description` alone).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub surface: Vec<Surface>,
    /// Recent per-merge outcome summaries from the project's journal
    /// ledger (RFC-36), newest activity last. Empty stays off the wire.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recent: Vec<String>,
    /// Accepted Decision Records projected from `.specify/decisions/`
    /// (RFC-36): the third routing-identity axis — *why* the project is
    /// shaped the way it is, surfaced so the agent can route a slice on
    /// architectural commitment and flag a lead that contradicts an
    /// accepted decision before Gate 1. Empty stays off the wire.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decisions: Vec<Decision>,
    /// Count of accepted decisions elided past the projection cap.
    /// Absent when the catalogue fits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decisions_more: Option<u64>,
}

/// One row in the request's flat lead catalog.
///
/// Identity is the `(source, lead)` pair; `lead` repeats
/// across rows when multiple sources surface the same slug. Mirrors a
/// single `discovery.md` [`specify_model::discovery::Lead`]
/// (RFC-29 D2; DECISIONS.md §"Lead reconciliation (D2)").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct LeadCatalogEntry {
    /// Plan source binding key matching `plan.yaml.sources.<key>`.
    pub source: String,
    /// Discovery lead id surfaced by this source binding.
    pub lead: String,
    /// Reconciliation-grade per-source headline — the primary signal for
    /// agent cross-source grouping. SHOULD name the operation/surface
    /// and its salient constraint so a same-slug lead from another
    /// source can be matched or distinguished on content.
    pub synopsis: String,
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

/// One `slices[]` row in a [`ProposalResponse`]: one slice of work
/// carrying its matched `sources[]` inline and its explicit `name`.
///
/// The `scope` noun was removed (RFC-29 review F3): there is no kernel
/// fan-out grouping. A body of work that targets more than one project
/// is expressed as multiple ordinary slices (which may legally reference
/// the same lead) joined by `depends-on`; the agent's explicit `name`
/// disambiguates cross-source matches that carry differing slugs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ResponseSlice {
    /// Explicit plan slice name (kebab-case). Required — with `scope`
    /// gone the agent names every slice directly, and the kernel writes
    /// it verbatim to `plan.yaml.slices[].name`.
    pub name: String,
    /// Matched catalog rows, each referenced by `{ source, lead }`
    /// (at most one per source). A lead may appear in more than one
    /// slice — that is fan-out.
    pub sources: Vec<ResponseMember>,
    /// Optional cross-source-match rationale the agent renders into
    /// `change.md` for Gate 1. Agent-authored and kernel-ignored — it is
    /// not echoed into the journal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rationale: Option<String>,
    /// Slice names this row depends on. Empty stays off the wire.
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
    pub source: String,
    /// Discovery lead id; with `source`, must match a request
    /// catalog row.
    pub lead: String,
}

/// Set of `(source, lead)` identities surveyed in `discovery.md`.
///
/// The membership oracle the response-validation kernel (a later chunk)
/// checks every agent-supplied `{ source, lead }` against to
/// reject orphan bindings and to prove every surveyed lead is covered by
/// at least one slice. Identities are deduplicated — a well-formed
/// `discovery.md` carries a unique `(source, lead)` per lead, so
/// [`LeadCatalog::len`] equals the surveyed lead count.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LeadCatalog {
    identities: BTreeSet<(String, String)>,
}

impl LeadCatalog {
    /// `true` when the `(source, lead)` identity was surveyed.
    #[must_use]
    pub fn contains(&self, source: &str, lead: &str) -> bool {
        self.identities.contains(&(source.to_owned(), lead.to_owned()))
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

/// Build the `(source, lead)` identity set from a surveyed
/// `discovery.md`.
///
/// Shared with the response-validation kernel: `propose --from`
/// re-reads `discovery.md`, calls this to rebuild the catalog, then
/// checks every response `(source, lead)` against it. Duplicate
/// identities collapse into one set entry (see [`LeadCatalog`]).
#[must_use]
pub fn build_catalog(discovery: &Discovery) -> LeadCatalog {
    LeadCatalog {
        identities: discovery
            .leads()
            .iter()
            .map(|lead| (lead.source.clone(), lead.lead.clone()))
            .collect(),
    }
}

/// Assemble the `kind: request` envelope from a surveyed `discovery.md`
/// and an already-resolved project topology.
///
/// `leads[]` is one [`LeadCatalogEntry`] per `discovery.leads()` row,
/// carrying `source`, `lead`, and `synopsis`.
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
            source: lead.source.clone(),
            lead: lead.lead.clone(),
            synopsis: lead.synopsis.clone(),
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
/// Two branches, keyed on the workspace-root discriminator
/// ([`ProjectConfig::workspace`]):
///
/// - **Workspace root** — one [`ProjectRef`] per entry in the committed
///   `.specify/topology.lock` (RFC-36), the projection of each member
///   project's `project.yaml` regenerated by `specrun workspace sync`.
///   `name`, `target`, `description`, `surface[]`, `decisions[]`, and
///   `recent[]` come from the cache. An absent cache fails `topology-cache-missing`
///   directing the operator to run `workspace sync`.
/// - **Single regular project** — one synthesised [`ProjectRef`]:
///   `name` from `project.yaml.name`, `description` from `project.yaml`,
///   `target` formed by resolving `project.yaml.adapter` through
///   [`TargetAdapter::resolve`], plus the live baseline projection
///   (`surface[]`, `decisions[]`, `recent[]`). A regular project reads its
///   own `project.yaml` live as its single source of truth — no cache.
///
/// Both branches touch the filesystem: the workspace branch reads the lock,
/// the regular branch resolves the target adapter under `project_dir`.
///
/// # Errors
///
/// - [`Error::Validation`] (`topology-cache-missing`) when a workspace root has no
///   committed `.specify/topology.lock`.
/// - [`Error::Validation`] (`plan-propose-project-adapter-missing`) when
///   a regular `project.yaml` omits `adapter` — a corrupt project that
///   `specrun init` never produces.
/// - Any error from [`TargetAdapter::resolve`] (`adapter-not-found`,
///   `adapter-schema-violation`, …) when the regular project's adapter
///   cannot be resolved.
pub fn resolve_topology(config: &ProjectConfig, project_dir: &Path) -> Result<Vec<ProjectRef>> {
    if config.workspace {
        workspace_topology(project_dir)
    } else {
        regular_topology(config, project_dir).map(|project| vec![project])
    }
}

/// Project every committed `.specify/topology.lock` entry into a
/// [`ProjectRef`]. RFC-36: workspace topology is derived from each member
/// project's `project.yaml`, not from `registry.yaml`.
fn workspace_topology(project_dir: &Path) -> Result<Vec<ProjectRef>> {
    let path = Layout::new(project_dir).topology_lock_path();
    let lock = TopologyLock::load(&path)?.ok_or_else(|| {
        Error::validation_failed(
            "topology-cache-missing",
            "a workspace root has a committed .specify/topology.lock",
            "workspace plan-time topology requires .specify/topology.lock; \
             run `specrun workspace sync` to regenerate it",
        )
    })?;
    Ok(lock
        .projects
        .into_iter()
        .map(|project| ProjectRef {
            name: project.name,
            target: project.target,
            description: project.description,
            surface: project.surface,
            recent: project.recent,
            decisions: project.decisions,
            decisions_more: project.decisions_more,
        })
        .collect())
}

/// Synthesise the sole [`ProjectRef`] for a single regular project.
fn regular_topology(config: &ProjectConfig, project_dir: &Path) -> Result<ProjectRef> {
    let adapter_value = config.adapter.as_deref().ok_or_else(|| {
        Error::validation_failed(
            "plan-propose-project-adapter-missing",
            "a regular project.yaml declares an adapter",
            "non-workspace project.yaml omits the `adapter` field",
        )
    })?;
    let resolved = TargetAdapter::resolve(adapter_name_from_value(adapter_value), project_dir)?;
    let target = format!("{}@v{}", resolved.manifest.name, resolved.manifest.version);
    let projection = crate::registry::identity::project_baseline(project_dir)?;
    Ok(ProjectRef {
        name: config.name.clone(),
        target,
        description: config.description.clone(),
        surface: projection.surface,
        recent: projection.recent,
        decisions: projection.decisions,
        decisions_more: projection.decisions_more,
    })
}

/// Outcome of a successful [`Plan::propose_from`] projection.
///
/// The reconciliation kernel returns it so the CLI handler (the
/// `propose --from` command) can emit the single D2
/// `plan.reconcile.completed` journal event without re-deriving
/// anything: [`ProposeOutcome::slice_names`] (plus its length) feeds the
/// payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposeOutcome {
    /// Slice names, in the agent's `slices[]` response order — the same
    /// order the kernel wrote `plan.yaml.slices[]`.
    pub slice_names: Vec<String>,
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
    /// violation: replaceable gate (`plan-reconcile-plan-not-replaceable`); lead-orphan (`plan-reconcile-lead-orphan`); per-slice same-source fusion (`plan-reconcile-slice-source-collision`); total lead coverage (`plan-reconcile-partition`); project auto-bind / orphan (`plan-reconcile-project-binding-required`, `plan-reconcile-project-orphan`); slice-name kebab-case + collision (`plan-reconcile-slice-name-invalid`, `plan-reconcile-slice-name-collision`); `depends-on` cycle (`plan-reconcile-depends-on-cycle`); and finally a backstop [`Plan::validate`] over the projected entries that rolls the plan back on any blocking finding.
    ///
    /// On success `self.entries` is the projected slice set in response
    /// order and the returned [`ProposeOutcome`] carries the slice names
    /// for the caller's `plan.reconcile.completed` journal event.
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
        let source_sets = slice_source_sets(slices)?;
        check_coverage(&source_sets, &catalog)?;
        let bound = bind_projects(slices, topology)?;
        // Eagerly validate that every bound project's target parses as
        // `name@vN` so a corrupt topology fails at propose time, even
        // though the resolved target is no longer written to disk.
        for project in &bound {
            parse_project_target(project)?;
        }
        let names = slice_names(slices)?;
        check_name_collisions(&names)?;

        let new_entries = build_entries(response.slices, &names, &bound);

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

        Ok(ProposeOutcome { slice_names: names })
    }
}

/// A `(source, lead)` catalog identity.
type LeadPair = (String, String);

/// The order-insensitive `sources[]` membership of one slice.
type SliceMembership = BTreeSet<LeadPair>;

/// Lead-orphan: every cited `(source, lead)` must name a current
/// catalog row (`plan-reconcile-lead-orphan`).
fn check_lead_orphans(slices: &[ResponseSlice], catalog: &LeadCatalog) -> Result<()> {
    for slice in slices {
        for member in &slice.sources {
            if !catalog.contains(&member.source, &member.lead) {
                return Err(Error::validation_failed(
                    "plan-reconcile-lead-orphan",
                    "every cited source binding must name a surveyed lead",
                    format!(
                        "({}, {}) is not in the discovery.md lead catalog",
                        member.source, member.lead
                    ),
                ));
            }
        }
    }
    Ok(())
}

/// Per-slice source set, enforcing at-most-one-lead-per-source
/// (`plan-reconcile-slice-source-collision`). This per-slice shape check
/// is independent of the removed `scope` grouping (RFC-29 review F3) — a
/// slice that named the same source twice is malformed regardless.
fn slice_source_sets(slices: &[ResponseSlice]) -> Result<Vec<SliceMembership>> {
    let mut out = Vec::with_capacity(slices.len());
    for slice in slices {
        let mut set = BTreeSet::new();
        let mut keys: HashSet<&str> = HashSet::new();
        for member in &slice.sources {
            if !keys.insert(member.source.as_str()) {
                return Err(Error::validation_failed(
                    "plan-reconcile-slice-source-collision",
                    "a slice names at most one lead per source",
                    format!(
                        "slice '{}' names source '{}' more than once",
                        slice.name, member.source
                    ),
                ));
            }
            set.insert((member.source.clone(), member.lead.clone()));
        }
        out.push(set);
    }
    Ok(out)
}

/// Total lead coverage: every surveyed lead is referenced by at least
/// one slice (`plan-reconcile-partition`). With the `scope` grouping
/// removed (RFC-29 review F3) a lead may legally appear in more than one
/// slice — that is fan-out, not a double-count — so coverage is the only
/// remaining invariant: nothing surveyed is left unplanned.
fn check_coverage(source_sets: &[SliceMembership], catalog: &LeadCatalog) -> Result<()> {
    let mut covered: HashSet<&LeadPair> = HashSet::new();
    for set in source_sets {
        for pair in set {
            covered.insert(pair);
        }
    }
    for pair in &catalog.identities {
        if !covered.contains(pair) {
            return Err(Error::validation_failed(
                "plan-reconcile-partition",
                "every surveyed lead must be referenced by at least one slice",
                format!("lead ({}, {}) is unaccounted for by any slice", pair.0, pair.1),
            ));
        }
    }
    Ok(())
}

/// Bind a single slice to its project in `topology`: an explicit
/// `project` must exist (`plan-reconcile-project-orphan`); an omitted
/// `project` auto-binds the sole project or fails when several exist
/// (`plan-reconcile-project-binding-required`). `slice_name` labels the
/// diagnostics.
///
/// This is the single project-binding rule shared by the propose kernel
/// ([`bind_projects`]) and the read-time target resolver
/// ([`resolve_target`]) so the two cannot drift (REVIEW.md A8).
fn resolve_project_binding<'a>(
    slice_name: &str, project: Option<&str>, topology: &'a [ProjectRef],
) -> Result<&'a ProjectRef> {
    match project {
        Some(name) => topology.iter().find(|p| p.name.as_str() == name).ok_or_else(|| {
            Error::validation_failed(
                "plan-reconcile-project-orphan",
                "a bound project must exist in the request topology",
                format!("slice '{slice_name}' binds unknown project '{name}'"),
            )
        }),
        None if topology.len() == 1 => Ok(&topology[0]),
        None => Err(Error::validation_failed(
            "plan-reconcile-project-binding-required",
            "a slice may omit project only when exactly one project exists",
            format!(
                "slice '{slice_name}' omits project but {} projects are available",
                topology.len()
            ),
        )),
    }
}

/// Bind each slice to a project via [`resolve_project_binding`].
fn bind_projects<'a>(
    slices: &[ResponseSlice], topology: &'a [ProjectRef],
) -> Result<Vec<&'a ProjectRef>> {
    slices
        .iter()
        .map(|slice| resolve_project_binding(&slice.name, slice.project.as_deref(), topology))
        .collect()
}

/// Resolve a single slice [`Entry`]'s target adapter from the project
/// topology.
///
/// A slice binds only a `project`; the target adapter (`name@vN`) is
/// derived here from the bound project's [`ProjectRef::target`]. This is
/// the single read-time resolver every consumer (`specrun plan next`,
/// slice `.metadata.yaml` population, the build request) routes through,
/// so `plan.yaml` never needs to store the denormalised target.
///
/// Binding mirrors the propose kernel's project binding: an explicit
/// [`Entry::project`] must exist in `topology`; an omitted project
/// auto-binds the sole topology project.
///
/// # Errors
///
/// - `plan-reconcile-project-orphan` when the named project is absent
///   from `topology`.
/// - `plan-reconcile-project-binding-required` when `project` is omitted
///   but more than one project exists.
/// - `plan-target-malformed` when the bound project's target does not
///   parse as `name@vN` (an internal inconsistency — topology targets
///   are pre-validated).
pub fn resolve_target(entry: &Entry, topology: &[ProjectRef]) -> Result<TargetRef> {
    let project = resolve_project_binding(&entry.name, entry.project.as_deref(), topology)?;
    parse_project_target(project)
}

/// Parse a [`ProjectRef`]'s `name@vN` target into a [`TargetRef`].
///
/// Topology targets are pre-validated, so a parse failure is an internal
/// inconsistency surfaced as `plan-target-malformed`.
fn parse_project_target(project: &ProjectRef) -> Result<TargetRef> {
    TargetRef::parse(&project.target).map_err(|err| {
        Error::validation_failed(
            "plan-target-malformed",
            "a project target must parse as name@vN",
            err.to_string(),
        )
    })
}

/// Collect each slice's explicit `name`, validating kebab-case
/// (`plan-reconcile-slice-name-invalid`). With `scope` removed (RFC-29
/// review F3) there is no kernel name derivation — the agent names every
/// slice and the kernel writes the name verbatim.
fn slice_names(slices: &[ResponseSlice]) -> Result<Vec<String>> {
    let mut names = Vec::with_capacity(slices.len());
    for slice in slices {
        if !is_kebab(&slice.name) {
            return Err(Error::validation_failed(
                "plan-reconcile-slice-name-invalid",
                "a slice name must be kebab-case",
                format!("slice name '{}' is not kebab-case", slice.name),
            ));
        }
        names.push(slice.name.clone());
    }
    Ok(names)
}

/// Reject clashing slice names (`plan-reconcile-slice-name-collision`).
/// With names now agent-supplied on every slice, this is the sole
/// uniqueness gate — it subsumes the former `(scope, project)`
/// duplicate check.
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
///
/// Each entry binds only its `project`; the target adapter is resolved
/// on demand from the topology via [`resolve_target`] and is not written
/// to disk.
fn build_entries(
    slices: Vec<ResponseSlice>, names: &[String], bound: &[&ProjectRef],
) -> Vec<Entry> {
    slices
        .into_iter()
        .enumerate()
        .map(|(idx, slice)| Entry {
            name: names[idx].clone(),
            project: Some(bound[idx].name.clone()),
            status: Status::Pending,
            depends_on: slice.depends_on,
            sources: slice
                .sources
                .into_iter()
                .map(|m| SliceSourceBinding::structured(m.source, m.lead))
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

#[cfg(test)]
mod tests;
