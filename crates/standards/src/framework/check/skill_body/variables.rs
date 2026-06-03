//! `$VAR` definition / reference coverage in the Arguments section.

use std::fs;
use std::sync::LazyLock;

use regex::Regex;
use specify_diagnostics::Diagnostic;

use super::{RULE_VARIABLE_COVERAGE, cached};
use crate::framework::builder::finding;
use crate::framework::context::Context;
use crate::framework::error::ToolingError;
use crate::framework::helpers::{relative_display, walk_skill_files};

static VAR_DEF_RE: LazyLock<Regex> = LazyLock::new(|| cached(r"(?m)^\$([A-Z_][A-Z_0-9]*)\s*="));
static VAR_USE_RE: LazyLock<Regex> = LazyLock::new(|| cached(r"\$([A-Z_][A-Z_0-9]*)"));
static ARGS_HEADING_RE: LazyLock<Regex> =
    LazyLock::new(|| cached(r"(?m)^## (?:Derived )?Arguments"));
static TEXT_CODE_BLOCK_RE: LazyLock<Regex> = LazyLock::new(|| cached(r"```text\n([\s\S]*?)```"));
static FENCE_STRIP_RE: LazyLock<Regex> = LazyLock::new(|| cached(r"```[\s\S]*?```"));
static INLINE_CODE_RE: LazyLock<Regex> = LazyLock::new(|| cached(r"`[^`]+`"));
static ALL_CAPS_VAR_RE: LazyLock<Regex> = LazyLock::new(|| cached(r"^[A-Z][A-Z_]+$"));

pub(super) fn check_variables(ctx: &Context) -> Result<Vec<Diagnostic>, ToolingError> {
    let def_re = &VAR_DEF_RE;
    let use_re = &VAR_USE_RE;
    let args_heading_re = &ARGS_HEADING_RE;
    let code_block_re = &TEXT_CODE_BLOCK_RE;
    let fence_re = &FENCE_STRIP_RE;
    let inline_code_re = &INLINE_CODE_RE;
    let all_caps_var = &ALL_CAPS_VAR_RE;

    let root = ctx.framework_root();
    let mut findings = Vec::new();

    for path in walk_skill_files(root)? {
        let rel = relative_display(root, &path);
        let content = fs::read_to_string(&path)?;

        let Some(heading_match) = args_heading_re.find(&content) else {
            continue;
        };
        let heading_idx = heading_match.start();
        let after_heading = &content[heading_match.end()..];
        let section_end = after_heading
            .find("\n## ")
            .map(|idx| heading_match.end() + idx)
            .unwrap_or(content.len());
        let args_section = &content[heading_idx..section_end];

        let mut defined = std::collections::HashSet::new();
        let mut used_in_defs = std::collections::HashSet::new();

        for block in code_block_re.captures_iter(args_section) {
            let block_text = block.get(1).map(|m| m.as_str()).unwrap_or("");
            for caps in def_re.captures_iter(block_text) {
                defined.insert(caps[1].to_string());
            }
            for line in block_text.split('\n') {
                let Some(eq_idx) = line.find('=') else {
                    continue;
                };
                let rhs = &line[eq_idx + 1..];
                for caps in use_re.captures_iter(rhs) {
                    let name = &caps[1];
                    if !is_builtin_var(name) {
                        used_in_defs.insert(name.to_string());
                    }
                }
            }
        }

        if defined.is_empty() {
            continue;
        }

        let body = &content[section_end..];
        let body_no_fences = fence_re.replace_all(body, "").into_owned();

        let used_in_body = collect_var_uses(&body_no_fences, use_re);
        let body_strict = inline_code_re.replace_all(&body_no_fences, "").into_owned();
        let used_in_body_strict = collect_var_uses(&body_strict, use_re);

        for var in &defined {
            if !used_in_body.contains(var) && !used_in_defs.contains(var) {
                findings.push(finding(
                    RULE_VARIABLE_COVERAGE,
                    format!("Unused variable: {rel} — ${var} defined but never referenced in body"),
                    Some(path.clone()),
                ));
            }
        }
        for var in &used_in_body_strict {
            if !defined.contains(var) && !is_builtin_var(var) && all_caps_var.is_match(var) {
                findings.push(finding(
                    RULE_VARIABLE_COVERAGE,
                    format!("Undefined variable: {rel} — ${var} used but not defined in Arguments"),
                    Some(path.clone()),
                ));
            }
        }
    }

    Ok(findings)
}

fn is_builtin_var(name: &str) -> bool {
    matches!(name, "ARGUMENTS" | "HOME")
}

fn collect_var_uses(text: &str, use_re: &Regex) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    for caps in use_re.captures_iter(text) {
        let name = caps[1].to_string();
        if !is_builtin_var(&name) {
            out.insert(name);
        }
    }
    out
}
