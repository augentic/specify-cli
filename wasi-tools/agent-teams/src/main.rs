//! `agent-teams` framework-authoring WASI tool entrypoint.
//!
//! Run under `specify lint framework`'s `kind: tool` evaluator. The
//! evaluator invokes the tool once per candidate file (a sentinel path,
//! since the overlay checks are whole-tree) and reads `PROJECT_DIR` from
//! the environment. The positional args carry the rule's own sentinel
//! path (e.g. `…/CORE-012-…md`) and its `config:` serialised as JSON. The
//! tool reads the `CORE-NNN` out of the sentinel to scope its output to
//! that one rule, and reads the canonical-document path (its only policy)
//! from the forwarded config, so no rule-specific literal is baked into
//! this binary.
//!
//! Findings are emitted on stdout as the shared
//! [`specify_framework_wire`] `DiagnosticReport` envelope; each carries
//! its own `rule-id: CORE-NNN` and `severity: important`. The host
//! restamps `id` and `fingerprint`. Exit is always `0` on a successful
//! run: the host treats a non-zero exit with no parsed findings as an
//! invocation failure, so a clean tree must exit `0`.

use std::path::PathBuf;
use std::process::ExitCode;

use specify_agent_teams::{AgentTeamsFinding, RULE_NON_CANONICAL, run};
use specify_framework_wire::{Row, parsed_config, print_report, requested_rule, string_field};

/// Every codex id this tool can emit, scanned for in the positional args
/// to scope a single invocation to one rule.
const RULES: &[&str] = &[RULE_NON_CANONICAL];

fn main() -> ExitCode {
    let Ok(project_dir) = std::env::var("PROJECT_DIR").map(PathBuf::from) else {
        print_report("agent-teams", []);
        return ExitCode::SUCCESS;
    };
    let args: Vec<String> = std::env::args().collect();
    let scoped = requested_rule(&args, RULES);
    let config = parsed_config(&args);
    let canonical_rel = string_field(config.as_ref(), "canonical-path");
    let findings: Vec<AgentTeamsFinding> = if canonical_rel.is_empty() {
        Vec::new()
    } else {
        run(&project_dir, &canonical_rel)
            .into_iter()
            .filter(|finding| scoped.is_none_or(|rule| finding.rule_id == rule))
            .collect()
    };
    print_report("agent-teams", findings.iter().map(row));
    ExitCode::SUCCESS
}

fn row(finding: &AgentTeamsFinding) -> Row<'_> {
    // Operator-facing impact / remediation prose for CORE-012 overlay
    // drift (the only rule this tool now emits).
    Row {
        rule_id: finding.rule_id,
        message: &finding.message,
        path: finding.path.as_deref(),
        impact: "A target adapter's agent-teams.md overlay does not match the canonical review-team-protocol document, so the overlay has drifted.",
        remediation: "Replace the overlay with a symlink to the canonical document, or re-sync its contents so the digests match.",
    }
}
