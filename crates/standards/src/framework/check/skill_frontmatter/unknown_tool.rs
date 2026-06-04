//! `skill.unknown-tool`: whitelist `allowed-tools` entries against the
//! known Cursor tool set.

use serde_json::Value as JsonValue;
use specify_diagnostics::Diagnostic;

use super::entries::load_skill_entries;
use super::{RULE_UNKNOWN_TOOL, finding};
use crate::framework::builder::infrastructure_finding;
use crate::framework::check::Check;
use crate::framework::context::Context;
use crate::framework::error::ToolingError;

/// Whitelist `allowed-tools` entries against the known Cursor tool set.
pub struct UnknownTool;

impl Check for UnknownTool {
    fn run(&self, ctx: &Context) -> Vec<Diagnostic> {
        check_unknown_tools(ctx)
            .unwrap_or_else(|error| vec![infrastructure_finding(RULE_UNKNOWN_TOOL, error)])
    }
}

fn check_unknown_tools(ctx: &Context) -> Result<Vec<Diagnostic>, ToolingError> {
    let mut findings = Vec::new();

    for entry in load_skill_entries(ctx)?.iter() {
        let Some(frontmatter) = &entry.frontmatter else {
            continue;
        };
        let Some(tools) = frontmatter.get("allowed-tools").and_then(JsonValue::as_str) else {
            continue;
        };

        for tool in tools.split_whitespace().map(str::trim).filter(|t| !t.is_empty()) {
            if known_tool(tool) {
                continue;
            }
            findings.push(finding(
                RULE_UNKNOWN_TOOL,
                format!("Unknown tool in allowed-tools: {} — '{tool}'", entry.rel),
                &entry.path,
            ));
        }
    }

    Ok(findings)
}

fn known_tool(tool: &str) -> bool {
    tool.starts_with("mcp__") || KNOWN_TOOLS.contains(&tool)
}

const KNOWN_TOOLS: &[&str] = &[
    "Read",
    "Write",
    "StrReplace",
    "Shell",
    "Grep",
    "Glob",
    "ReadLints",
    "WebFetch",
    "WebSearch",
    "AskQuestion",
    "Task",
    "TodoWrite",
    "SemanticSearch",
    "EditNotebook",
    "GenerateImage",
];
