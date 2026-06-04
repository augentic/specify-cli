//! `skill.argument-hint-grammar`: enforce the canonical `argument-hint:`
//! token grammar with rich diagnostics.

use regex::Regex;
use specify_diagnostics::Diagnostic;

use super::entries::load_skill_entries;
use super::{RULE_ARGUMENT_HINT_GRAMMAR, finding};
use crate::framework::builder::infrastructure_finding;
use crate::framework::check::Check;
use crate::framework::context::Context;
use crate::framework::error::ToolingError;

/// Enforce the canonical `argument-hint:` token grammar with rich diagnostics.
pub struct ArgumentHintGrammar;

impl Check for ArgumentHintGrammar {
    fn run(&self, ctx: &Context) -> Vec<Diagnostic> {
        check_argument_hint_grammar(ctx)
            .unwrap_or_else(|error| vec![infrastructure_finding(RULE_ARGUMENT_HINT_GRAMMAR, error)])
    }
}

fn check_argument_hint_grammar(ctx: &Context) -> Result<Vec<Diagnostic>, ToolingError> {
    let mut findings = Vec::new();

    for entry in load_skill_entries(ctx)?.iter() {
        let Some(frontmatter) = &entry.frontmatter else {
            continue;
        };
        let Some(hint_value) = frontmatter.get("argument-hint") else {
            continue;
        };
        let Some(hint) = hint_value.as_str() else {
            findings.push(finding(
                RULE_ARGUMENT_HINT_GRAMMAR,
                format!("Invalid argument-hint type in {}: must be a string", entry.rel),
                &entry.path,
            ));
            continue;
        };

        if let Some(message) = argument_hint_grammar_error(hint) {
            findings.push(finding(
                RULE_ARGUMENT_HINT_GRAMMAR,
                format!(
                    "Invalid argument-hint in {}: token '{message}' (in '{hint}') does not match grammar — allowed tokens are <name>, [name], <a|b>, [a|b], <name>..., [name]..., --flag (kebab-case names)",
                    entry.rel
                ),
                &entry.path,
            ));
        }
    }

    Ok(findings)
}

/// Pure per-hint predicate for tests and richer diagnostics than JSON Schema alone.
pub fn argument_hint_grammar_error(hint: &str) -> Option<String> {
    let trimmed = hint.trim();
    if trimmed.is_empty() {
        return None;
    }

    let token_re = argument_hint_token_regex();
    for token in trimmed.split_whitespace() {
        if !token_re.is_match(token) {
            return Some(token.to_string());
        }
    }
    None
}

fn argument_hint_token_regex() -> Regex {
    let hint_name = r"[a-z][a-z0-9]*(?:-[a-z0-9]+)*";
    let hint_alt = format!("{hint_name}(?:\\|{hint_name})*");
    Regex::new(&format!(
        "^(?:<{hint_alt}>(?:\\.\\.\\.)?|\\[{hint_alt}\\](?:\\.\\.\\.)?|--[a-z][a-z0-9]*(?:-[a-z0-9]+)*)$"
    ))
    .expect("argument-hint token regex")
}
