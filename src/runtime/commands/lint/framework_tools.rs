//! Name-resolved tool runner for `specify lint framework` (Road B; see
//! DECISIONS.md §"Framework lint engine: generic dispatcher (Road A / Road B)").
//!
//! Framework runs have no `project.yaml` to populate a tool inventory, so
//! the first-party framework checkers are declared here as a closed
//! inventory keyed by name. `is_declared` / `run` dispatch by name — the
//! standards engine never calls a checker directly, which is the
//! decoupling lever the standards-layer split requires: the engine sees
//! only the [`ToolRunner`] trait and folds each checker's
//! `DiagnosticReport` envelope, exactly as it did when the checkers were
//! out-of-process WASI components (the B-2 exit replaced the Wasmtime
//! hop with an in-process call; the wire shape, name resolution, and
//! rule-owned `config:` policy forwarding are unchanged).

mod links_registry;
mod marketplace;
mod prose;
mod rules;
mod scenarios;
mod skill_body;
mod support;

use std::path::Path;

use specify_standards::lint::eval::tool::{ToolOutput, ToolRunError, ToolRunner};

use self::support::ToolFinding;

/// One in-process framework checker: its declared name and entry point.
struct FrameworkTool {
    name: &'static str,
    run: fn(&Path, &[String]) -> Vec<ToolFinding>,
}

/// Closed inventory of framework checkers `specify lint framework`
/// resolves by name. Grows one row per Road B family tool.
const FRAMEWORK_TOOLS: &[FrameworkTool] = &[
    FrameworkTool {
        name: "scenarios",
        run: scenarios::run,
    },
    FrameworkTool {
        name: "skill-body",
        run: skill_body::run,
    },
    FrameworkTool {
        name: "links-registry",
        run: links_registry::run,
    },
    FrameworkTool {
        name: "marketplace",
        run: marketplace::run,
    },
    FrameworkTool {
        name: "prose",
        run: prose::run,
    },
    FrameworkTool {
        name: "rules",
        run: rules::run,
    },
];

fn lookup(name: &str) -> Option<&'static FrameworkTool> {
    FRAMEWORK_TOOLS.iter().find(|tool| tool.name == name)
}

/// Name-resolving [`ToolRunner`] for the framework surface.
#[derive(Debug, Default)]
pub struct FrameworkToolRunner;

impl ToolRunner for FrameworkToolRunner {
    fn is_declared(&self, tool_name: &str) -> bool {
        lookup(tool_name).is_some()
    }

    fn run(
        &self, tool_name: &str, args: &[String], project_dir: &Path,
    ) -> Result<ToolOutput, ToolRunError> {
        let Some(tool) = lookup(tool_name) else {
            return Err(ToolRunError::Runtime(format!(
                "tool {tool_name} is not a declared framework checker"
            )));
        };
        let findings = (tool.run)(project_dir, args);
        Ok(support::report_output(&findings))
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use specify_diagnostics::DiagnosticReport;
    use specify_standards::lint::eval::tool::ToolRunner;

    use super::FrameworkToolRunner;

    #[test]
    fn declares_exactly_the_six_checkers() {
        let runner = FrameworkToolRunner;
        for name in ["scenarios", "skill-body", "links-registry", "marketplace", "prose", "rules"] {
            assert!(runner.is_declared(name), "{name} must be declared");
        }
        assert!(!runner.is_declared("agent-teams"), "agent-teams retired with CORE-012");
        assert!(!runner.is_declared("contract"), "adapter tools stay WASI-resolved");
    }

    #[test]
    fn run_emits_parseable_report_envelope() {
        let dir = tempfile::tempdir().expect("tempdir");
        let runner = FrameworkToolRunner;
        let output = runner
            .run("marketplace", &["sentinel.md".to_string()], dir.path())
            .expect("marketplace runs");
        assert_eq!(output.exit_code, 0);
        let report: DiagnosticReport =
            serde_json::from_slice(&output.stdout).expect("stdout is a DiagnosticReport");
        // An empty tree has no marketplace.json: exactly one drift finding.
        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].rule_id.as_deref(), Some("CORE-022"));
    }

    #[test]
    fn undeclared_tool_is_a_runtime_error() {
        let runner = FrameworkToolRunner;
        let err = runner.run("ghost", &[], Path::new(".")).expect_err("ghost is undeclared");
        assert!(err.to_string().contains("not a declared framework checker"));
    }
}
