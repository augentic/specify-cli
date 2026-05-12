//! Specify change orchestration: plan-driven multi-slice changes, the
//! operator-facing `change.md` brief, the `plan.yaml` state machine,
//! and the closure verb `specify change finalize`.

pub mod finalize;
mod plan;

pub use finalize::summarise;
pub use plan::core::{Entry, EntryPatch, Finding, Patch, Plan, Severity, Status};
pub use plan::doctor::{
    BlockingPredecessor, CYCLE, CloneSignature, Diagnostic as PlanDoctorDiagnostic,
    DiagnosticPayload as PlanDoctorPayload, DiagnosticSeverity as PlanDoctorSeverity,
    ORPHAN_SOURCE, STALE_CLONE, StaleReason, UNREACHABLE, doctor as plan_doctor,
};
pub use plan::lock::{
    Acquired, Guard, Released as PlanLockReleased, Stamp, State as PlanLockState,
};
