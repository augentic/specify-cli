//! Specify change orchestration: plan-driven multi-slice changes, the
//! operator-facing `change.md` brief, the `plan.yaml` state machine,
//! and the closure verb `specify change finalize`.

pub mod finalize;
mod plan;

pub use finalize::summarise;
pub use plan::core::{
    Divergence, Entry, EntryPatch, Finding, Lifecycle, Patch, Plan, Severity,
    SliceAuthorityOverride, SliceSourceBinding, Status, authority_override_orphan_source_keys,
};
pub use plan::doctor::{
    CYCLE, CloneSignature, Diagnostic as PlanDoctorDiagnostic,
    DiagnosticPayload as PlanDoctorPayload, ORPHAN_SOURCE, STALE_CLONE, StaleReason,
    doctor as plan_doctor,
};
pub use plan::lock::{Acquired, Released as PlanLockReleased, Stamp, State as PlanLockState};
