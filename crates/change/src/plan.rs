//! Plan orchestration primitives.
//!
//! Three submodules: the `plan.yaml` state machine (`core`), the
//! `specify change plan doctor` health diagnostics (`doctor`), and the
//! advisory PID lock at `.specify/plan.lock` (`lock`).
//!
//! Lifted from `crates/slice/src/{plan, plan_doctor, lock}.rs` by
//! RFC-13 chunk 2.4. The umbrella orchestration crate now owns these
//! primitives; `specify-slice` (the per-loop unit) keeps only the
//! per-slice `.metadata.yaml` lifecycle, the per-slice journal,
//! and the kebab-name validator.

pub mod core;
pub mod doctor;
pub mod lock;
