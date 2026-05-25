//! Plan orchestration primitives: the `plan.yaml` state machine
//! (`core`) and the four health diagnostics surfaced through
//! `specrun plan validate` (`doctor`).

pub(super) mod core;
pub(super) mod doctor;
