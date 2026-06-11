//! `skill-body` framework-authoring WASI tool entrypoint.
//!
//! Run under `specify lint framework`'s `kind: tool` evaluator. The
//! evaluator invokes the tool once per candidate file (a sentinel path,
//! since the body checks walk the whole `plugins/` tree) and reads
//! `PROJECT_DIR` from the environment. The positional args carry the
//! rule's own sentinel path (e.g. `…/CORE-040-…md`) and — when the rule
//! declares one — its `config:` serialised as JSON. The tool reads the
//! `CORE-NNN` out of the sentinel to scope its output to that one rule,
//! and reads its policy (line threshold, item bounds, built-in variable
//! allow-list) from the forwarded config, so no rule-specific literal is
//! baked into this binary.
//!
//! Findings are emitted on stdout as the shared
//! [`specify_framework_wire`] `DiagnosticReport` envelope; each carries
//! its own `rule-id: CORE-NNN` and `severity: important`. The host
//! restamps `id` and `fingerprint`. Exit is always `0` on a successful
//! run.

use std::path::PathBuf;
use std::process::ExitCode;

use specify_framework_wire::{
    Row, parsed_config, print_report, requested_rule, string_array_field, usize_field,
};
use specify_skill_body::{
    RULE_INVALID_CRITICAL_PATH, RULE_STEP_BODY_DUPLICATES, RULE_VARIABLE_COVERAGE,
    SkillBodyFinding, check_invalid_critical_path, check_step_body_duplicates,
    check_variable_coverage,
};

const RULES: &[&str] =
    &[RULE_INVALID_CRITICAL_PATH, RULE_STEP_BODY_DUPLICATES, RULE_VARIABLE_COVERAGE];

fn main() -> ExitCode {
    let Ok(project_dir) = std::env::var("PROJECT_DIR").map(PathBuf::from) else {
        print_report("skill-body", []);
        return ExitCode::SUCCESS;
    };
    let args: Vec<String> = std::env::args().collect();
    let scoped = requested_rule(&args, RULES);
    let config = parsed_config(&args);
    let findings = match scoped {
        Some(RULE_INVALID_CRITICAL_PATH) => check_invalid_critical_path(
            &project_dir,
            usize_field(config.as_ref(), "min-body-lines"),
            usize_field(config.as_ref(), "min-items"),
            usize_field(config.as_ref(), "max-items"),
        ),
        Some(RULE_STEP_BODY_DUPLICATES) => check_step_body_duplicates(&project_dir),
        Some(RULE_VARIABLE_COVERAGE) => check_variable_coverage(
            &project_dir,
            &string_array_field(config.as_ref(), "builtin-vars"),
        ),
        _ => Vec::new(),
    };
    print_report("skill-body", findings.iter().map(row));
    ExitCode::SUCCESS
}

fn row(finding: &SkillBodyFinding) -> Row<'_> {
    let (impact, remediation) = guidance(finding.rule_id);
    Row {
        rule_id: finding.rule_id,
        message: &finding.message,
        path: Some(&finding.path),
        impact,
        remediation,
    }
}

fn guidance(rule_id: &str) -> (&'static str, &'static str) {
    match rule_id {
        RULE_INVALID_CRITICAL_PATH => (
            "A long skill's `## Critical Path` section does not list the required number of steps, so the table of contents is not a faithful map of the skill body.",
            "Rewrite the `## Critical Path` section to list the configured number of bullets or numbered steps.",
        ),
        RULE_STEP_BODY_DUPLICATES => (
            "A step body repeats a `## Critical Path` entry verbatim, duplicating the table of contents instead of pointing to references.",
            "Keep step bodies as short pointers to references; do not restate the Critical Path entries.",
        ),
        _ => (
            "A skill defines a `$VAR` in its Arguments section that is never referenced (or references one that is never defined), so the variable contract is inconsistent.",
            "Reference every defined `$VAR` in the body and define every `$VAR` the body uses in the Arguments section.",
        ),
    }
}
