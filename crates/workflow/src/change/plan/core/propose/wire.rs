//! Lead-reconciliation envelope DTOs (RFC-29 D2).
//!
//! The wire contract is a single envelope discriminated by a closed
//! `kind: request | response`, validated against
//! `schemas/discovery/proposal.schema.json`
//! ([`crate::schema::validate_proposal_json`]). This module owns the
//! serde DTOs for both kinds; the deterministic assembly that fills them
//! lives in [`super::catalog`] and [`super::topology`].

use serde::{Deserialize, Serialize};

use crate::registry::topology::{Decision, Surface};

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
/// Emitted by `specify plan propose --dry-run --format json`: a flat
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
/// For a workspace this is projected from the committed
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
    /// `omnia@v1`). Resolved on demand by [`super::resolve_target`] for a
    /// slice bound to this project; it is no longer written to
    /// `plan.yaml` (a slice stores only its `project`).
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
    /// Target platforms this project builds for, projected from
    /// `project.yaml.platforms`. Empty stays off the wire.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub platforms: Vec<crate::Platform>,
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
/// Consumed by `specify plan propose --from`. The DTO is shape-only; the
/// partition, fan-out, project-binding, and name-derivation invariants
/// are enforced by the projection kernel (`Plan::propose_from`), not by
/// serde.
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
