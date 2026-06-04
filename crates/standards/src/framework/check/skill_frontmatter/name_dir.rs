//! `skill.name-directory-mismatch` (`CORE-043`): require `name:` to
//! carry the containing plugin's discovery prefix.

use regex::Regex;
use serde_json::Value as JsonValue;
use specify_diagnostics::Diagnostic;

use super::entries::load_skill_entries;
use super::{RULE_NAME_DIRECTORY_MISMATCH, finding};
use crate::framework::builder::infrastructure_finding;
use crate::framework::check::Check;
use crate::framework::context::Context;
use crate::framework::error::ToolingError;

const PREFIX_OVERRIDES: &[(&str, &str)] = &[("spec", "specify")];

/// Require `name:` to carry the containing plugin's discovery prefix.
pub struct NameDirMismatch;

impl Check for NameDirMismatch {
    fn run(&self, ctx: &Context) -> Vec<Diagnostic> {
        check_name_directory_mismatch(ctx).unwrap_or_else(|error| {
            vec![infrastructure_finding(RULE_NAME_DIRECTORY_MISMATCH, error)]
        })
    }
}

fn check_name_directory_mismatch(ctx: &Context) -> Result<Vec<Diagnostic>, ToolingError> {
    let name_re = Regex::new(r"^[a-z][a-z0-9-]*$").expect("name regex");
    let mut findings = Vec::new();

    for entry in load_skill_entries(ctx)?.iter() {
        let Some(frontmatter) = &entry.frontmatter else {
            continue;
        };

        let Some(name) = frontmatter.get("name").and_then(JsonValue::as_str) else {
            continue;
        };

        if !name_re.is_match(name) {
            continue;
        }

        let prefix_base = prefix_override(&entry.plugin_dir).unwrap_or(entry.plugin_dir.as_str());
        let required_prefix = format!("{prefix_base}-");
        if name.starts_with(&required_prefix) {
            continue;
        }

        findings.push(finding(
            RULE_NAME_DIRECTORY_MISMATCH,
            format!(
                "Skill name missing plugin prefix: {} — '{name}' must start with '{required_prefix}'",
                entry.rel
            ),
            &entry.path,
        ));
    }

    Ok(findings)
}

fn prefix_override(plugin_dir: &str) -> Option<&'static str> {
    PREFIX_OVERRIDES.iter().find(|(dir, _)| *dir == plugin_dir).map(|(_, prefix)| *prefix)
}
