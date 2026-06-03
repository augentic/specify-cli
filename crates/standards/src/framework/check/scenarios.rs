use specify_diagnostics::Diagnostic;

use crate::framework::check::Check;
use crate::framework::context::Context;

mod discovery;
mod frontmatter;
mod trace;

pub use frontmatter::validate_scenario_frontmatter;
pub use trace::check_recorded_trace_freshness;

pub const RULE_SCHEMA_VIOLATION: &str = "scenarios.schema-violation";
pub const RULE_STAGES_NOT_CONTIGUOUS: &str = "scenarios.stages-not-contiguous-prefix";
pub const RULE_BODY_ID_MISMATCH: &str = "scenarios.body-id-mismatch";
pub const RULE_ARTIFACT_PATH_UNSAFE: &str = "scenarios.artifact-path-unsafe";
pub const RULE_DUPLICATE_ID: &str = "scenarios.duplicate-id";
pub const RULE_RECORDED_TRACE_VIOLATION: &str = "scenarios.recorded-trace-violation";
pub const RULE_STALE_RECORDED_TRACE: &str = "scenarios.stale-recorded-trace";

/// Scenario frontmatter validation and recorded-trace freshness checks.
pub struct ScenariosCheck;

impl Check for ScenariosCheck {
    fn run(&self, ctx: &Context) -> Vec<Diagnostic> {
        let mut findings = validate_scenario_frontmatter(ctx);
        findings.extend(check_recorded_trace_freshness(ctx));
        findings
    }
}
