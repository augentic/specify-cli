//! Plan orchestration primitives: the `plan.yaml` state machine
//! (`core`), the `specify change plan doctor` health diagnostics
//! (`doctor`), and the advisory PID lock (`lock`).

pub(crate) mod core;
pub(crate) mod doctor;
pub(crate) mod lock;
