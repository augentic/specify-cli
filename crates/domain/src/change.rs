//! Specify change orchestration: plan-driven multi-slice changes, the
//! operator-facing `change.md` brief, and the `plan.yaml` state machine.

mod plan;

pub use plan::core::{
    Divergence, Entry, EntryPatch, Finding, Lifecycle, Patch, Plan, Severity,
    SliceAuthorityOverride, SliceSourceBinding, SourceBinding, Status, TargetRef,
    TargetRefParseError, authority_override_orphan_source_keys,
    emit_authority_override_seed_events, entry_mut, mutate_authority_overrides,
    refuse_orphan_authority_overrides, unknown_slice_err,
};
pub use plan::doctor::{
    CYCLE, CloneSignature, Diagnostic as PlanDoctorDiagnostic,
    DiagnosticPayload as PlanDoctorPayload, ORPHAN_SOURCE, STALE_CLONE, StaleReason, detect,
    doctor as plan_doctor,
};
