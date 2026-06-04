//! `skill.missing-frontmatter` (`CORE-042`) and `skill.schema-violation`
//! (`CORE-044`): validate SKILL.md frontmatter against
//! `schemas/authoring/skill.schema.json`.

use specify_diagnostics::Diagnostic;

use super::entries::load_skill_entries;
use super::{RULE_MISSING_FRONTMATTER, RULE_SCHEMA_VIOLATION, finding};
use crate::framework::builder::infrastructure_finding;
use crate::framework::check::Check;
use crate::framework::context::Context;
use crate::framework::error::ToolingError;
use crate::framework::schema::{SchemaError, SchemaId, validate_frontmatter};

/// Validate SKILL.md frontmatter against `schemas/authoring/skill.schema.json`.
pub struct FrontmatterSchema;

impl Check for FrontmatterSchema {
    fn run(&self, ctx: &Context) -> Vec<Diagnostic> {
        check_schema(ctx)
            .unwrap_or_else(|error| vec![infrastructure_finding(RULE_SCHEMA_VIOLATION, error)])
    }
}

fn check_schema(ctx: &Context) -> Result<Vec<Diagnostic>, ToolingError> {
    let mut findings = Vec::new();
    findings.extend(findings_missing_frontmatter(ctx)?);
    findings.extend(findings_schema_violation(ctx)?);
    Ok(findings)
}

/// RFC-31 Phase 2 de-fuse: missing-frontmatter findings only (`CORE-042`).
pub fn findings_missing_frontmatter(ctx: &Context) -> Result<Vec<Diagnostic>, ToolingError> {
    schema_findings_for_rule(ctx, RULE_MISSING_FRONTMATTER)
}

/// RFC-31 Phase 2 de-fuse: schema-violation findings only (`CORE-044`).
pub fn findings_schema_violation(ctx: &Context) -> Result<Vec<Diagnostic>, ToolingError> {
    schema_findings_for_rule(ctx, RULE_SCHEMA_VIOLATION)
}

fn schema_findings_for_rule(ctx: &Context, rule_id: &str) -> Result<Vec<Diagnostic>, ToolingError> {
    let mut findings = Vec::new();

    for entry in load_skill_entries(ctx)?.iter() {
        match validate_frontmatter(&entry.path, SchemaId::Skill) {
            Ok(()) => {}
            Err(SchemaError::Infrastructure(error)) => {
                return Err(error);
            }
            Err(SchemaError::Validation(errors)) => {
                for error in errors {
                    let mapped = if error.message.contains("missing leading YAML frontmatter") {
                        RULE_MISSING_FRONTMATTER
                    } else {
                        RULE_SCHEMA_VIOLATION
                    };
                    if mapped != rule_id {
                        continue;
                    }
                    findings.push(finding(
                        mapped,
                        format!(
                            "Skill frontmatter: {} — {} {}",
                            entry.rel, error.instance_path, error.message
                        ),
                        &entry.path,
                    ));
                }
            }
        }
    }

    Ok(findings)
}
