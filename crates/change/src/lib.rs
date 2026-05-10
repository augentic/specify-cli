#![allow(
    clippy::multiple_crate_versions,
    reason = "specify-config (re-exports `specify_tool::Tool`) transitively pulls in Wasmtime/WASI duplicate versions."
)]

//! Specify change orchestration.
//!
//! Plan-driven multi-slice changes, the operator-facing `change.md`
//! brief, the `plan.yaml` state machine, and the closure verb
//! `specify change finalize`.
//!
//! Lifted out of the binary lib (`src/initiative_finalize.rs`) and
//! the per-loop unit crate (`crates/slice/src/{plan,plan_doctor,
//! lock}.rs`) by RFC-13 chunk 2.4 and renamed from `specify-initiative`
//! to `specify-change` by RFC-13 chunk 3.4. RFC-13 chunk 3.5 reshaped the
//! action enums under `specify change`, and chunk 3.7 migrated the on-disk
//! brief from `initiative.md` to `change.md`.
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

pub mod finalize;
mod plan;

pub use finalize::summarise;
pub use plan::core::{Entry, EntryPatch, Finding, Plan, Severity, Status};
pub use plan::doctor::{
    BlockingPredecessor, CYCLE, CloneSignature, Diagnostic as PlanDoctorDiagnostic,
    DiagnosticPayload as PlanDoctorPayload, DiagnosticSeverity as PlanDoctorSeverity,
    ORPHAN_SOURCE, STALE_CLONE, StaleReason, UNREACHABLE, doctor as plan_doctor,
};
pub use plan::lock::{Acquired, Guard, PlanLockReleased, PlanLockState, Stamp};
