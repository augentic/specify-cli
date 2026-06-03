//! Inline-JSON length and CLI-envelope-shape checks on skill bodies.

use std::fs;
use std::sync::LazyLock;

use regex::{Regex, RegexBuilder};
use specify_diagnostics::Diagnostic;

use super::{RULE_ENVELOPE_JSON_IN_BODY, RULE_INLINE_JSON_TOO_LONG, cached};
use crate::framework::builder::finding;
use crate::framework::context::Context;
use crate::framework::error::ToolingError;
use crate::framework::helpers::{relative_display, walk_skill_files};

static INLINE_JSON_FENCE_RE: LazyLock<Regex> = LazyLock::new(|| cached(r"^```(json|jsonc)\b"));
static ENVELOPE_FENCE_RE: LazyLock<Regex> = LazyLock::new(|| cached(r"^\s*(`{3,})(json|jsonc)\b"));
static ENVELOPE_VERSION_RE: LazyLock<Regex> =
    LazyLock::new(|| cached(r#""envelope[-_]version"\s*:"#));
static ENVELOPE_OK_RE: LazyLock<Regex> = LazyLock::new(|| cached(r#""ok"\s*:\s*(true|false)\b"#));
static ENVELOPE_DATA_RE: LazyLock<Regex> = LazyLock::new(|| cached(r#""data"\s*:"#));
static ENVELOPE_ERROR_RE: LazyLock<Regex> = LazyLock::new(|| cached(r#""error"\s*:\s*\{"#));

const MAX_INLINE_JSON_LINES: usize = 30;

pub(super) fn check_inline_json_blocks(ctx: &Context) -> Result<Vec<Diagnostic>, ToolingError> {
    let fence_open_re = &INLINE_JSON_FENCE_RE;
    let root = ctx.framework_root();
    let mut findings = Vec::new();

    for path in walk_skill_files(root)? {
        let rel = relative_display(root, &path);
        let content = fs::read_to_string(&path)?;
        let lines: Vec<&str> = content.split('\n').collect();

        let mut in_block = false;
        let mut block_start = 0usize;
        let mut block_length = 0usize;

        for (i, line) in lines.iter().enumerate() {
            if !in_block {
                if fence_open_re.is_match(line) {
                    in_block = true;
                    block_start = i + 1;
                    block_length = 0;
                }
                continue;
            }
            if line.starts_with("```") {
                if block_length > MAX_INLINE_JSON_LINES {
                    findings.push(finding(
                        RULE_INLINE_JSON_TOO_LONG,
                        format!(
                            "Inline JSON too long: {rel}:{} — {block_length} body lines (limit {MAX_INLINE_JSON_LINES}); move large output shapes to docs/reference/cli-output-shapes.md and link to them",
                            block_start + 1
                        ),
                        Some(path.clone()),
                    ));
                }
                in_block = false;
                continue;
            }
            block_length += 1;
        }
    }

    Ok(findings)
}

pub(super) fn check_no_envelope_examples(ctx: &Context) -> Result<Vec<Diagnostic>, ToolingError> {
    let fence_open_re = &ENVELOPE_FENCE_RE;
    let root = ctx.framework_root();
    let mut findings = Vec::new();

    for path in walk_skill_files(root)? {
        let rel = relative_display(root, &path);
        let content = fs::read_to_string(&path)?;
        let lines: Vec<&str> = content.split('\n').collect();

        let mut in_block = false;
        let mut block_start = 0usize;
        let mut block_body: Vec<String> = Vec::new();
        let mut open_fence: Option<String> = None;
        let mut violations = Vec::new();

        for (i, line) in lines.iter().enumerate() {
            if !in_block {
                if let Some(caps) = fence_open_re.captures(line) {
                    in_block = true;
                    open_fence = Some(caps[1].to_string());
                    block_start = i + 1;
                    block_body.clear();
                }
                continue;
            }

            let fence = open_fence.as_deref().unwrap_or("```");
            let close_re = RegexBuilder::new(&format!(r"^\s*{fence}\s*$"))
                .build()
                .expect("fence close pattern");
            if close_re.is_match(line) {
                if is_envelope_body(&block_body) {
                    violations.push(block_start + 1);
                }
                in_block = false;
                open_fence = None;
                block_body.clear();
                continue;
            }
            block_body.push((*line).to_string());
        }

        if !violations.is_empty() {
            let where_str =
                violations.iter().map(|n| format!("line {n}")).collect::<Vec<_>>().join(", ");
            findings.push(finding(
                RULE_ENVELOPE_JSON_IN_BODY,
                format!(
                    "Envelope JSON in skill body: {rel} — {} block(s) at {where_str} (link to docs/reference/cli-output-shapes.md instead of embedding the envelope shape)",
                    violations.len()
                ),
                Some(path),
            ));
        }
    }

    Ok(findings)
}

fn is_envelope_body(body: &[String]) -> bool {
    let text = body.join("\n");
    if ENVELOPE_VERSION_RE.is_match(&text) {
        return true;
    }
    let has_ok = ENVELOPE_OK_RE.is_match(&text);
    let has_data = ENVELOPE_DATA_RE.is_match(&text);
    let has_error = ENVELOPE_ERROR_RE.is_match(&text);
    has_ok && (has_data || has_error)
}
