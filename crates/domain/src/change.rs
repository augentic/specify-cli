
//! Specify change orchestration.
//!
//! Plan-driven multi-slice changes, the operator-facing `change.md`
//! brief, the `plan.yaml` state machine, and the closure verb
//! `specify change finalize`.
//!
//! Dependency direction:
//!
//! ```text
//! specify-change → specify-registry → specify-capability
//!               → specify-slice    (per-loop unit primitives)
//!               → specify-error
//! ```
//!
//! `specify-slice` MUST NOT depend on this crate; the per-loop unit
//! is the substrate, the umbrella orchestration is the consumer.

pub mod finalize;
mod plan;

pub use finalize::summarise;
pub use plan::core::{Entry, EntryPatch, Finding, Patch, Plan, Severity, Status};
pub use plan::doctor::{
    BlockingPredecessor, CYCLE, CloneSignature, Diagnostic as PlanDoctorDiagnostic,
    DiagnosticPayload as PlanDoctorPayload, DiagnosticSeverity as PlanDoctorSeverity,
    ORPHAN_SOURCE, STALE_CLONE, StaleReason, UNREACHABLE, doctor as plan_doctor,
};
pub use plan::lock::{Acquired, Guard, PlanLockReleased, PlanLockState, Stamp};
