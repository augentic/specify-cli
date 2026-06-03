//! Per-section line-budget and frontmatter-restatement checks.

use std::fs;

use specify_diagnostics::Diagnostic;

use super::RULE_SECTION_LINE_COUNT;
use crate::framework::builder::finding;
use crate::framework::context::Context;
use crate::framework::error::ToolingError;
use crate::framework::helpers::{relative_display, skill_body_lines, walk_skill_files};

const MAX_SECTION_LINES: usize = 45;

pub(super) fn check_section_line_counts(ctx: &Context) -> Result<Vec<Diagnostic>, ToolingError> {
    let root = ctx.framework_root();
    let mut findings = Vec::new();

    for path in walk_skill_files(root)? {
        let rel = relative_display(root, &path);
        let content = fs::read_to_string(&path)?;
        let Some(lines) = skill_body_lines(&content) else {
            continue;
        };

        let h2_indices: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter(|(_, line)| line.starts_with("## "))
            .map(|(idx, _)| idx)
            .collect();

        let mut violations = Vec::new();
        for (i, &start) in h2_indices.iter().enumerate() {
            let end = h2_indices.get(i + 1).copied().unwrap_or(lines.len());
            let title = lines[start][3..].trim();
            let section_lines = &lines[start + 1..end];
            let cnt = count_section_body_lines(section_lines);
            if cnt > MAX_SECTION_LINES {
                violations.push(format!("'{title}' ({cnt} lines)"));
            }
        }

        if !violations.is_empty() {
            findings.push(finding(
                RULE_SECTION_LINE_COUNT,
                format!(
                    "Skill section too long: {rel} — {} section(s) over {MAX_SECTION_LINES} lines: {} (move depth into references/ and link from the H2)",
                    violations.len(),
                    violations.join(", ")
                ),
                Some(path),
            ));
        }
    }

    Ok(findings)
}

fn count_section_body_lines(section_lines: &[String]) -> usize {
    let mut count = 0;
    let mut in_fence = false;
    for line in section_lines {
        if line.starts_with("```") {
            in_fence = !in_fence;
            count += 1;
            continue;
        }
        if in_fence {
            count += 1;
            continue;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with("<!--") && trimmed.ends_with("-->") {
            continue;
        }
        count += 1;
    }
    count
}
