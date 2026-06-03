//! Inline-JSON length and CLI-envelope-shape checks on skill bodies.

use std::fs;
use std::sync::LazyLock;

use regex::Regex;
use specify_diagnostics::Diagnostic;

use super::{RULE_INLINE_JSON_TOO_LONG, cached};
use crate::framework::builder::finding;
use crate::framework::context::Context;
use crate::framework::error::ToolingError;
use crate::framework::helpers::{relative_display, walk_skill_files};

static INLINE_JSON_FENCE_RE: LazyLock<Regex> = LazyLock::new(|| cached(r"^```(json|jsonc)\b"));
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
