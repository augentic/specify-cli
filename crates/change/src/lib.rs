//! Specify change orchestration.
//!
//! Plan-driven multi-change loops, the operator-facing `initiative.md`
//! brief, the `plan.yaml` state machine, and the closure verb
//! `specify initiative finalize`.
//!
//! Lifted out of the binary lib (`src/initiative_finalize.rs`) and
//! the per-loop unit crate (`crates/slice/src/{plan,plan_doctor,
//! lock}.rs`) by RFC-13 chunk 2.4 and renamed from `specify-initiative`
//! to `specify-change` by RFC-13 chunk 3.4. The on-disk `initiative.md`
//! and `Commands::Initiative` CLI surface still carry the pre-rename
//! noun — chunk 3.5 reshapes the action enums and chunk 3.7 migrates
//! the on-disk file.
//!
//! Dependency direction (RFC-13 invariant #4):
//!
//! ```text
//! specify-change → specify-registry → specify-capability
//!               → specify-slice    (per-loop unit primitives)
//!               → specify-error
//! ```
//!
//! `specify-slice` MUST NOT depend on this crate; the per-loop unit
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
