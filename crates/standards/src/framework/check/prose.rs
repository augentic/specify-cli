use std::fs;
use std::path::Path;

use specify_diagnostics::Diagnostic;

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
    let files: [(&str, bool, bool); 2] = [
        (".cursor/schemas/skill.schema.json", true, false),
        ("docs/standards/skill-authoring.md", true, true),
    ];

    let mut findings = Vec::new();
    for (rel, checks_description, checks_body) in files {
        let path = framework_root.join(rel);
        let content = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(_) => {
                findings.push(framework_finding(
                    RULE_NUMERIC_CAP_EXCEEDED,
                    format!("Skill numeric cap source missing: {rel}"),
                    Some(loc(path.clone(), 1, None)),
                ));
                continue;
            }
        };

        if checks_description && !content.contains(&EXPECTED_DESCRIPTION_CAP.to_string()) {
            findings.push(framework_finding(
                RULE_NUMERIC_CAP_EXCEEDED,
                format!(
                    "Skill description cap drift in {rel}; expected {EXPECTED_DESCRIPTION_CAP}"
                ),
                Some(loc(path.clone(), 1, None)),
            ));
        }
        if checks_body && !content.contains(&EXPECTED_BODY_CAP.to_string()) {
            findings.push(framework_finding(
                RULE_NUMERIC_CAP_EXCEEDED,
                format!("Skill body cap drift in {rel}; expected {EXPECTED_BODY_CAP}"),
                Some(loc(path.clone(), 1, None)),
            ));
        }
    }

    findings
}
