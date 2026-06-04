use std::fs;
use std::path::Path;

use specify_diagnostics::Diagnostic;
use specify_schema::SKILL_JSON_SCHEMA;

use crate::framework::builder::{framework_finding, loc};
use crate::framework::check::Check;
use crate::framework::context::Context;

const RULE_NUMERIC_CAP_EXCEEDED: &str = "prose.numeric-cap-exceeded";

const EXPECTED_DESCRIPTION_CAP: usize = 512;
const EXPECTED_BODY_CAP: usize = 200;

/// Skill description/body numeric caps must stay in sync across schema, standards, and checks.
pub struct NumericCaps;

impl Check for NumericCaps {
    fn run(&self, ctx: &Context) -> Vec<Diagnostic> {
        check_skill_numeric_caps(ctx.framework_root())
    }
}

fn check_skill_numeric_caps(framework_root: &Path) -> Vec<Diagnostic> {
    let mut findings = Vec::new();

    // The canonical skill schema is embedded in the binary; cross-check
    // the description cap against it rather than a vendored editor mirror.
    if !SKILL_JSON_SCHEMA.contains(&EXPECTED_DESCRIPTION_CAP.to_string()) {
        findings.push(framework_finding(
            RULE_NUMERIC_CAP_EXCEEDED,
            format!(
                "Skill description cap drift in embedded skill.schema.json; \
                 expected {EXPECTED_DESCRIPTION_CAP}"
            ),
            None,
        ));
    }

    // The standards doc carries both caps in prose and must agree.
    let rel = "docs/standards/skill-authoring.md";
    let path = framework_root.join(rel);
    match fs::read_to_string(&path) {
        Ok(content) => {
            if !content.contains(&EXPECTED_DESCRIPTION_CAP.to_string()) {
                findings.push(framework_finding(
                    RULE_NUMERIC_CAP_EXCEEDED,
                    format!(
                        "Skill description cap drift in {rel}; expected {EXPECTED_DESCRIPTION_CAP}"
                    ),
                    Some(loc(path.clone(), 1, None)),
                ));
            }
            if !content.contains(&EXPECTED_BODY_CAP.to_string()) {
                findings.push(framework_finding(
                    RULE_NUMERIC_CAP_EXCEEDED,
                    format!("Skill body cap drift in {rel}; expected {EXPECTED_BODY_CAP}"),
                    Some(loc(path.clone(), 1, None)),
                ));
            }
        }
        Err(_) => {
            findings.push(framework_finding(
                RULE_NUMERIC_CAP_EXCEEDED,
                format!("Skill numeric cap source missing: {rel}"),
                Some(loc(path.clone(), 1, None)),
            ));
        }
    }

    findings
}
