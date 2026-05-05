//! Specify initiative orchestration.
//!
//! Plan-driven multi-change loops, the operator-facing `initiative.md`
//! brief, the `plan.yaml` state machine, and the closure verb
//! `specify initiative finalize`.
//!
//! Lifted out of the binary lib (`src/initiative_finalize.rs`) and
//! the per-loop unit crate (`crates/change/src/{plan,plan_doctor,
//! lock}.rs`) by RFC-13 chunk 2.4. The crate name is a placeholder —
//! Phase 3.4 will rename it to `specify-change` once the per-loop
//! unit crate (currently `crates/change/`) is renamed to `slice`.
//!
//! Dependency direction (RFC-13 invariant #4):
//!
//! ```text
//! specify-initiative → specify-registry → specify-capability
//!                   → specify-change   (per-loop unit primitives)
//!                   → specify-error
//! ```
//!
//! `specify-change` MUST NOT depend on this crate; the per-loop unit
//! is the substrate, the umbrella orchestration is the consumer.

/// `specify initiative finalize` — RFC-9 §4C closure verb.
pub mod finalize;
/// `plan.yaml` state machine, plan-doctor, advisory plan lock.
pub mod plan;

pub use finalize::{
    FinalizeError, FinalizeInputs, FinalizeOutcome, FinalizeProbe, FinalizeProjectResult,
    FinalizeStatus, FinalizeSummaryCounts, RealFinalizeProbe, classify_pr_state, combine_status,
    is_terminal_for_finalize, load_plan_or_refuse, non_terminal_entries, run_finalize, summarise,
};
pub use plan::core::{Entry, EntryPatch, Finding, Plan, Severity, Status};
pub use plan::doctor::{
    BlockingPredecessor, CODE_CYCLE, CODE_ORPHAN_SOURCE, CODE_STALE_CLONE, CODE_UNREACHABLE,
    CloneSignature, Diagnostic as PlanDoctorDiagnostic, DiagnosticPayload as PlanDoctorPayload,
    DiagnosticSeverity as PlanDoctorSeverity, StaleCloneReason, doctor as plan_doctor,
};
pub use plan::lock::{Acquired, Guard, PlanLockReleased, PlanLockState, Stamp};
