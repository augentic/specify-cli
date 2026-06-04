//! `skill.description-grammar`: require `description:` to start with a
//! curated imperative verb.

use serde_json::Value as JsonValue;
use specify_diagnostics::Diagnostic;

use super::entries::load_skill_entries;
use super::{RULE_DESCRIPTION_GRAMMAR, finding};
use crate::framework::builder::infrastructure_finding;
use crate::framework::check::Check;
use crate::framework::context::Context;
use crate::framework::error::ToolingError;

/// Require `description:` to start with a curated imperative verb.
pub struct DescriptionGrammar;

impl Check for DescriptionGrammar {
    fn run(&self, ctx: &Context) -> Vec<Diagnostic> {
        check_description_grammar(ctx)
            .unwrap_or_else(|error| vec![infrastructure_finding(RULE_DESCRIPTION_GRAMMAR, error)])
    }
}

fn check_description_grammar(ctx: &Context) -> Result<Vec<Diagnostic>, ToolingError> {
    let mut findings = Vec::new();

    for entry in load_skill_entries(ctx)?.iter() {
        let Some(frontmatter) = &entry.frontmatter else {
            continue;
        };
        let Some(description) = frontmatter.get("description").and_then(JsonValue::as_str) else {
            continue;
        };

        let trimmed = description.trim_start();
        let Some(first_word) = trimmed.split_whitespace().next() else {
            findings.push(finding(
                RULE_DESCRIPTION_GRAMMAR,
                format!(
                    "Skill description must start with an imperative verb: {} — no leading word found",
                    entry.rel
                ),
                &entry.path,
            ));
            continue;
        };

        let first_alpha =
            first_word.chars().take_while(|ch| ch.is_ascii_alphabetic()).collect::<String>();
        if first_alpha.is_empty() {
            findings.push(finding(
                RULE_DESCRIPTION_GRAMMAR,
                format!(
                    "Skill description must start with an imperative verb: {} — no leading word found",
                    entry.rel
                ),
                &entry.path,
            ));
            continue;
        }

        let lower = first_alpha.to_ascii_lowercase();
        if imperative_verbs().contains(&lower.as_str()) {
            continue;
        }

        findings.push(finding(
            RULE_DESCRIPTION_GRAMMAR,
            format!(
                "Skill description must start with an imperative verb: {} — '{first_alpha}' not in allow-list (add to IMPERATIVE_VERBS in crates/standards/src/framework/check/skill_frontmatter/description.rs if it is genuinely imperative)", 
                entry.rel
            ),
            &entry.path,
        ));
    }

    Ok(findings)
}

fn imperative_verbs() -> &'static [&'static str] {
    &[
        "add",
        "annotate",
        "apply",
        "audit",
        "author",
        "build",
        "categorise",
        "categorize",
        "check",
        "compare",
        "compile",
        "complete",
        "compose",
        "compute",
        "configure",
        "convert",
        "create",
        "decompose",
        "define",
        "describe",
        "design",
        "diff",
        "discover",
        "drive",
        "drop",
        "enforce",
        "execute",
        "expose",
        "export",
        "extract",
        "fetch",
        "fix",
        "format",
        "generate",
        "guard",
        "implement",
        "import",
        "infer",
        "ingest",
        "init",
        "initialize",
        "list",
        "load",
        "merge",
        "monitor",
        "orchestrate",
        "plan",
        "preview",
        "process",
        "produce",
        "propose",
        "publish",
        "reconstruct",
        "refine",
        "render",
        "resolve",
        "review",
        "run",
        "scaffold",
        "select",
        "show",
        "shorten",
        "split",
        "stage",
        "store",
        "summarize",
        "test",
        "translate",
        "transform",
        "trim",
        "validate",
        "verify",
        "wire",
        "wrap",
        "write",
    ]
}
