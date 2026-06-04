//! Specify change orchestration: plan-driven multi-slice changes, the
//! operator-facing `change.md` brief, and the `plan.yaml` state machine.

mod plan;

pub use plan::core::{
    Divergence, Entry, EntryPatch, LeadCatalog, LeadCatalogEntry, Lifecycle, NextBody, NextReason,
    Patch, Plan, ProjectMissingPlatforms, ProjectRef, ProposalKind, ProposalRequest,
    ProposalResponse, ProposeOutcome, ResponseMember, ResponseSlice, SliceAuthorityOverride,
    SliceSourceBinding, SourceBinding, Status, TargetRef, TargetRefParseError, build_catalog,
    build_request, detect_missing_platforms, emit_authority_override_seed_events, entry_mut,
    mutate_authority_overrides, orphan_authority_override_keys, plan_finding,
    plan_finding_structured, plan_next_body, reject_orphan_overrides, resolve_target,
    resolve_topology, unknown_slice_err,
};
pub use plan::doctor::{
    CYCLE, CloneSignature, ORPHAN_SOURCE, STALE_CLONE, StaleReason, detect, doctor as plan_doctor,
};
