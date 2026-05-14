//! Plan orchestration primitives: the `plan.yaml` state machine
//! (`core`), the `specify change plan doctor` health diagnostics
//! (`doctor`), and the advisory PID lock (`lock`).

pub(super) mod core;
pub(super) mod doctor;
pub(super) mod lock;
