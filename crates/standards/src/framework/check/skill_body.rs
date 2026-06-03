use regex::Regex;
use specify_diagnostics::Diagnostic;

use crate::framework::builder::infrastructure_finding;
use crate::framework::check::Check;
use crate::framework::context::Context;

mod critical_path;
mod envelope;
mod section;
mod variables;

/// Build a process-wide cached regex. The patterns here are static literals
/// authored in these modules, so a compile failure is a programmer error caught
/// by the unit tests below, not a runtime condition.
pub(super) fn cached(pattern: &str) -> Regex {
    Regex::new(pattern)
        .unwrap_or_else(|err| unreachable!("static skill-body regex must compile: {err}"))
}

pub(super) const RULE_SECTION_LINE_COUNT: &str = "skill.section-line-count";
pub(super) const RULE_MISSING_CRITICAL_PATH: &str = "skill.missing-critical-path";
pub(super) const RULE_INVALID_CRITICAL_PATH: &str = "skill.invalid-critical-path";
pub(super) const RULE_INLINE_JSON_TOO_LONG: &str = "skill.inline-json-too-long";
pub(super) const RULE_STEP_BODY_DUPLICATES: &str = "skill.step-body-duplicates-critical-path";
pub(super) const RULE_VARIABLE_COVERAGE: &str = "skill.variable-coverage";

/// Each H2 section must stay within the per-section line budget.
pub struct SectionLineCount;

/// Long skills must include a `## Critical Path` block.
pub struct MissingCriticalPath;

/// Critical Path must list 5–7 steps (list or H3 form).
pub struct InvalidCriticalPath;

/// Inline `json` / `jsonc` fences must not exceed 30 body lines.
pub struct InlineJsonTooLong;

/// Step bodies must not duplicate Critical Path entries verbatim.
pub struct StepBodyDuplicatesCriticalPath;

/// `$VAR`s in Arguments must be defined and referenced consistently.
pub struct VariableCoverage;

macro_rules! impl_skill_body_check {
    ($ty:ty, $rule:expr, $body:expr) => {
        impl Check for $ty {
            fn run(&self, ctx: &Context) -> Vec<Diagnostic> {
                $body(ctx).unwrap_or_else(|error| vec![infrastructure_finding($rule, error)])
            }
        }
    };
}

impl_skill_body_check!(
    SectionLineCount,
    RULE_SECTION_LINE_COUNT,
    section::check_section_line_counts
);
impl_skill_body_check!(
    MissingCriticalPath,
    RULE_MISSING_CRITICAL_PATH,
    critical_path::check_missing_critical_path
);
impl_skill_body_check!(
    InvalidCriticalPath,
    RULE_INVALID_CRITICAL_PATH,
    critical_path::check_invalid_critical_path
);
impl_skill_body_check!(
    InlineJsonTooLong,
    RULE_INLINE_JSON_TOO_LONG,
    envelope::check_inline_json_blocks
);
impl_skill_body_check!(
    StepBodyDuplicatesCriticalPath,
    RULE_STEP_BODY_DUPLICATES,
    critical_path::check_step_body_vs_critical_path
);
impl_skill_body_check!(VariableCoverage, RULE_VARIABLE_COVERAGE, variables::check_variables);
