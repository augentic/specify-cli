//! Plan orchestration primitives.
//!
//! Three submodules: the `plan.yaml` state machine (`core`), the
//! `specify change plan doctor` health diagnostics (`doctor`), and the
//! advisory PID lock at `.specify/plan.lock` (`lock`).
//!
//! The umbrella orchestration crate owns these primitives;
//! `specify-slice` (the per-loop unit) keeps only the per-slice
//! `.metadata.yaml` lifecycle, the per-slice journal, and the
//! kebab-name validator.

pub(crate) mod core;
pub(crate) mod doctor;
pub(crate) mod lock;
