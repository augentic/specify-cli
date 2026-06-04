//! Recorded-trace header validation and best-effort staleness hints.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

use serde_json::Value as JsonValue;
use specify_diagnostics::Diagnostic;
use walkdir::WalkDir;

use super::{RULE_RECORDED_TRACE_VIOLATION, RULE_STALE_RECORDED_TRACE};
use crate::framework::builder::finding;
use crate::framework::context::Context;
use crate::framework::helpers::{relative_display, under_symlink};

const TRACE_REQUIRED_FIELDS: [&str; 6] =
    ["kind", "schemaVersion", "sourceBackend", "sourceRunId", "sourceTimestamp", "scenarioId"];

/// Run recorded-trace header validation and best-effort recency hints.
pub fn check_recorded_trace_freshness(ctx: &Context) -> Vec<Diagnostic> {
    let recorded_root = ctx.framework_root().join("acceptance").join("recorded");
    if !recorded_root.is_dir() {
        return Vec::new();
    }

    let mut trace_paths = Vec::new();
    for entry in WalkDir::new(&recorded_root).follow_links(false).into_iter().flatten() {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.into_path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
            continue;
        }
        if under_symlink(ctx.framework_root(), &path).unwrap_or(true) {
            continue;
        }
        trace_paths.push(path);
    }
    trace_paths.sort();

    let mut findings = Vec::new();
    let mut headers_by_path: HashMap<PathBuf, JsonValue> = HashMap::new();

    for path in &trace_paths {
        let rel = relative_display(ctx.framework_root(), path);
        let content = match fs::read_to_string(path) {
            Ok(content) => content,
            Err(source) => {
                findings.push(finding(
                    RULE_RECORDED_TRACE_VIOLATION,
                    format!("Recorded trace: {rel} — cannot read: {source}"),
                    Some(path.clone()),
                ));
                continue;
            }
        };

        let first_line = content.lines().next().unwrap_or("").trim();
        if first_line.is_empty() {
            findings.push(finding(
                RULE_RECORDED_TRACE_VIOLATION,
                format!(
                    "Recorded trace: {rel} — empty file (expected a 'recorded-trace-header' line first)"
                ),
                Some(path.clone()),
            ));
            continue;
        }

        let parsed: JsonValue = match serde_json::from_str(first_line) {
            Ok(value) => value,
            Err(source) => {
                findings.push(finding(
                    RULE_RECORDED_TRACE_VIOLATION,
                    format!("Recorded trace: {rel} — first line is not valid JSON: {source}"),
                    Some(path.clone()),
                ));
                continue;
            }
        };

        if !parsed.is_object() {
            findings.push(finding(
                RULE_RECORDED_TRACE_VIOLATION,
                format!("Recorded trace: {rel} — first line must be a JSON object"),
                Some(path.clone()),
            ));
            continue;
        }

        let header = parsed.clone();
        let kind = header.get("kind").and_then(JsonValue::as_str);
        if kind != Some("recorded-trace-header") {
            findings.push(finding(
                RULE_RECORDED_TRACE_VIOLATION,
                format!(
                    "Recorded trace: {rel} — first line kind must be 'recorded-trace-header' (got {})",
                    serde_json::to_string(header.get("kind").unwrap_or(&JsonValue::Null))
                        .unwrap_or_else(|_| "<unknown>".into())
                ),
                Some(path.clone()),
            ));
            continue;
        }

        let schema_version = header.get("schemaVersion");
        if schema_version != Some(&JsonValue::Number(1.into())) {
            findings.push(finding(
                RULE_RECORDED_TRACE_VIOLATION,
                format!(
                    "Recorded trace: {rel} — recorded-trace-header.schemaVersion must be 1 (got {})",
                    serde_json::to_string(schema_version.unwrap_or(&JsonValue::Null))
                        .unwrap_or_else(|_| "<unknown>".into())
                ),
                Some(path.clone()),
            ));
        }

        for field in TRACE_REQUIRED_FIELDS {
            let value = header.get(field);
            let missing = match value {
                None | Some(JsonValue::Null) => true,
                Some(JsonValue::String(s)) => s.is_empty(),
                _ => false,
            };
            if missing {
                findings.push(finding(
                    RULE_RECORDED_TRACE_VIOLATION,
                    format!(
                        "Recorded trace: {rel} — recorded-trace-header missing required field '{field}'"
                    ),
                    Some(path.clone()),
                ));
            }
        }

        headers_by_path.insert(path.clone(), header);
    }

    emit_stale_trace_hints(ctx, &trace_paths, &headers_by_path);

    findings.sort_by(|a, b| a.title.cmp(&b.title));
    findings
}

fn emit_stale_trace_hints(
    ctx: &Context, trace_paths: &[PathBuf], headers_by_path: &HashMap<PathBuf, JsonValue>,
) {
    let output = Command::new("git")
        .args(["diff", "--name-only", "HEAD~1..HEAD"])
        .current_dir(ctx.framework_root())
        .output();

    let Ok(output) = output else {
        return;
    };
    if !output.status.success() {
        return;
    }

    let diff: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect();

    let traces_by_rel: HashMap<String, &PathBuf> = trace_paths
        .iter()
        .map(|path| (relative_display(ctx.framework_root(), path), path))
        .collect();

    for rel in diff {
        let Some(path) = traces_by_rel.get(&rel) else {
            continue;
        };
        let header = headers_by_path.get(*path);
        let run_id = header
            .and_then(|h| h.get("sourceRunId"))
            .and_then(JsonValue::as_str)
            .unwrap_or("<unknown>");
        let ts = header
            .and_then(|h| h.get("sourceTimestamp"))
            .and_then(JsonValue::as_str)
            .unwrap_or("<unknown>");
        eprintln!(
            "WARN: {RULE_STALE_RECORDED_TRACE}: Recorded trace updated in HEAD: {rel} — \
             consider quoting sourceRunId='{run_id}' / sourceTimestamp='{ts}' in the commit \
             message so reviewers can trace it back to the live run."
        );
    }
}
