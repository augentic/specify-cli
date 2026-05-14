//! Plan orchestration primitives: the `plan.yaml` state machine
//! (`core`), the four health diagnostics surfaced through
//! `specify change plan validate` (`doctor`), and the advisory PID
//! lock (`lock`).

pub(super) mod core;
pub(super) mod doctor;
pub(super) mod lock;
