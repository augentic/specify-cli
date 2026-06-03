//! `## Critical Path` presence, shape, and step-body duplication checks.

use std::fs;
use std::sync::LazyLock;

use regex::Regex;
use specify_diagnostics::Diagnostic;

use super::{
    RULE_INVALID_CRITICAL_PATH, RULE_MISSING_CRITICAL_PATH, RULE_STEP_BODY_DUPLICATES, cached,
};
use crate::framework::builder::finding;
use crate::framework::context::Context;
use crate::framework::error::ToolingError;
use crate::framework::helpers::{relative_display, skill_body_lines, walk_skill_files};

static LIST_ITEM_RE: LazyLock<Regex> = LazyLock::new(|| cached(r"^(?:\d+\.|-)\s+\S"));
static STEP_PREFIX_RE: LazyLock<Regex> = LazyLock::new(|| cached(r"^Step\s+\d+\s*[:.\-]\s*"));
static LIST_PREFIX_RE: LazyLock<Regex> = LazyLock::new(|| cached(r"^(?:\d+\.|-|\*)\s+"));
static HEADING_PREFIX_RE: LazyLock<Regex> = LazyLock::new(|| cached(r"^#{2,4}\s+"));
static WHITESPACE_RE: LazyLock<Regex> = LazyLock::new(|| cached(r"\s+"));
static LIST_LINE_RE: LazyLock<Regex> = LazyLock::new(|| cached(r"^(?:\d+\.|-|\*)\s+\S"));

const CRITICAL_PATH_MIN_LINES: usize = 150;
const CRITICAL_PATH_HEADING: &str = "## Critical Path";

pub(super) fn check_missing_critical_path(ctx: &Context) -> Result<Vec<Diagnostic>, ToolingError> {
    let root = ctx.framework_root();
    let mut findings = Vec::new();

    for path in walk_skill_files(root)? {
        let rel = relative_display(root, &path);
        let content = fs::read_to_string(&path)?;
        let Some(lines) = skill_body_lines(&content) else {
            continue;
        };

        if lines.len() < CRITICAL_PATH_MIN_LINES {
            continue;
        }

        let has_heading = lines.iter().any(|line| line.trim() == CRITICAL_PATH_HEADING);
        if !has_heading {
            findings.push(finding(
                RULE_MISSING_CRITICAL_PATH,
                format!(
                    "Missing Critical Path: {rel} — {} body lines requires '{CRITICAL_PATH_HEADING}'",
                    lines.len()
                ),
                Some(path),
            ));
        }
    }

    Ok(findings)
}

pub(super) fn check_invalid_critical_path(ctx: &Context) -> Result<Vec<Diagnostic>, ToolingError> {
    let root = ctx.framework_root();
    let mut findings = Vec::new();

    for path in walk_skill_files(root)? {
        let rel = relative_display(root, &path);
        let content = fs::read_to_string(&path)?;
        let Some(lines) = skill_body_lines(&content) else {
            continue;
        };

        if lines.len() < CRITICAL_PATH_MIN_LINES {
            continue;
        }

        let Some(heading_index) =
            lines.iter().position(|line| line.trim() == CRITICAL_PATH_HEADING)
        else {
            continue;
        };

        let section_lines = critical_path_section_lines(&lines, heading_index);
        let item_count = count_critical_path_items(section_lines, &LIST_ITEM_RE);

        if !(5..=7).contains(&item_count) {
            findings.push(finding(
                RULE_INVALID_CRITICAL_PATH,
                format!(
                    "Invalid Critical Path: {rel} — expected 5-7 bullets or numbered items, found {item_count}"
                ),
                Some(path),
            ));
        }
    }

    Ok(findings)
}

fn critical_path_section_lines(lines: &[String], heading_index: usize) -> &[String] {
    let rest = &lines[heading_index + 1..];
    if let Some(next_h2) = rest.iter().position(|line| line.starts_with("## ")) {
        &rest[..next_h2]
    } else {
        rest
    }
}

fn count_critical_path_items(section_lines: &[String], list_item_re: &Regex) -> usize {
    let mut item_count = 0;
    let mut mode: Option<CriticalPathMode> = None;

    for line in section_lines {
        let trimmed = line.trim();
        if mode.is_none() {
            if trimmed.is_empty() {
                continue;
            }
            if line.starts_with("### ") {
                mode = Some(CriticalPathMode::H3);
                item_count += 1;
                continue;
            }
            if list_item_re.is_match(line) {
                mode = Some(CriticalPathMode::List);
                item_count += 1;
            }
            continue;
        }
        match mode {
            Some(CriticalPathMode::H3) if line.starts_with("### ") => {
                item_count += 1;
            }
            Some(CriticalPathMode::H3) => {}
            Some(CriticalPathMode::List) => {
                if trimmed.is_empty() {
                    break;
                }
                if list_item_re.is_match(line) {
                    item_count += 1;
                }
            }
            None => {}
        }
    }

    item_count
}

#[derive(Clone, Copy)]
enum CriticalPathMode {
    H3,
    List,
}

pub(super) fn check_step_body_vs_critical_path(
    ctx: &Context,
) -> Result<Vec<Diagnostic>, ToolingError> {
    let root = ctx.framework_root();
    let mut findings = Vec::new();

    for path in walk_skill_files(root)? {
        let rel = relative_display(root, &path);
        let content = fs::read_to_string(&path)?;
        let Some(lines) = skill_body_lines(&content) else {
            continue;
        };

        let Some(cp_start) = lines.iter().position(|line| line.trim() == "## Critical Path") else {
            continue;
        };

        let cp_end = cp_start
            + 1
            + lines[cp_start + 1..]
                .iter()
                .position(|line| line.starts_with("## "))
                .unwrap_or(lines.len() - cp_start - 1);

        let cp_entries = collect_critical_path_entries(&lines, cp_start + 1, cp_end);
        if cp_entries.is_empty() {
            continue;
        }

        let violations = find_step_body_duplicates(&lines, cp_end, &cp_entries);
        if violations.is_empty() {
            continue;
        }

        let detail: Vec<String> = violations
            .iter()
            .take(3)
            .map(|(line, text)| format!("line {line}: '{}'", truncate(text, 80)))
            .collect();
        let more = if violations.len() > 3 {
            format!(" (+{} more)", violations.len() - 3)
        } else {
            String::new()
        };
        findings.push(finding(
            RULE_STEP_BODY_DUPLICATES,
            format!(
                "Step body duplicates Critical Path: {rel} — {} match(es): {}{} (Critical Path is the TOC; keep step bodies as short pointers to references)",
                violations.len(),
                detail.join("; "),
                more
            ),
            Some(path),
        ));
    }

    Ok(findings)
}

fn collect_critical_path_entries(
    lines: &[String], start: usize, end: usize,
) -> std::collections::HashSet<String> {
    let mut cp_entries = std::collections::HashSet::new();
    let mut in_fence = false;
    for line in &lines[start..end] {
        if line.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence || !is_list_or_heading_line(line) {
            continue;
        }
        let norm = normalise_entry(line);
        if !norm.is_empty() {
            cp_entries.insert(norm);
        }
    }
    cp_entries
}

fn find_step_body_duplicates(
    lines: &[String], cp_end: usize, cp_entries: &std::collections::HashSet<String>,
) -> Vec<(usize, String)> {
    let mut violations = Vec::new();
    let mut in_fence = false;
    for (i, raw) in lines.iter().enumerate().skip(cp_end) {
        if raw.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence || !is_list_or_heading_line(raw) {
            continue;
        }
        let norm = normalise_entry(raw);
        if norm.is_empty() {
            continue;
        }
        if cp_entries.contains(&norm) {
            violations.push((i + 1, raw.trim().to_string()));
        }
    }
    violations
}

fn normalise_entry(text: &str) -> String {
    let mut out = text.to_string();
    out = LIST_PREFIX_RE.replace_all(&out, "").into_owned();
    out = HEADING_PREFIX_RE.replace_all(&out, "").into_owned();
    out = STEP_PREFIX_RE.replace_all(&out, "").into_owned();
    out = WHITESPACE_RE.replace_all(out.trim(), " ").into_owned();
    out.to_lowercase()
}

fn is_list_or_heading_line(line: &str) -> bool {
    LIST_LINE_RE.is_match(line) || line.starts_with("### ") || line.starts_with("#### ")
}

fn truncate(text: &str, max: usize) -> String {
    if text.len() <= max { text.to_string() } else { text[..max].to_string() }
}
