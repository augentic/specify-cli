//! Rust-source quality checks when `specdev lint` targets the specify-cli repo.

use std::fs;
use std::path::Path;

use specify_diagnostics::Diagnostic;

use crate::framework::builder::{framework_finding, loc};
use crate::framework::check::Check;
use crate::framework::context::Context;
use crate::framework::helpers::relative_display;

const RULE_ARCHAEOLOGY: &str = "rust.archaeology-in-doc-comment";
const RULE_ALLOW_NO_REASON: &str = "rust.allow-without-reason";

const ARCHAEOLOGY_MARKERS: &[&str] = &[
    "RFC-",
    "Phase ",
    "formerly ",
    "previously lived",
    "old contract",
    "pre-cutover",
    "folded pair",
];

/// Runs when the framework root is specify-cli (`crates/workflow` + `src/runtime`).
pub struct RustSourceQuality;

impl Check for RustSourceQuality {
    fn run(&self, ctx: &Context) -> Vec<Diagnostic> {
        let root = ctx.framework_root();
        if !root.join("crates/workflow").is_dir() || !root.join("src/runtime").is_dir() {
            return Vec::new();
        }

        let mut findings = Vec::new();
        for sub in ["crates", "src"] {
            let dir = root.join(sub);
            if dir.is_dir() {
                walk_rust_sources(root, &dir, &mut findings);
            }
        }
        findings
    }
}

fn walk_rust_sources(root: &Path, dir: &Path, findings: &mut Vec<Diagnostic>) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n == "target" || n == "wasi-tools") {
                continue;
            }
            walk_rust_sources(root, &path, findings);
            continue;
        }
        if path.extension().is_some_and(|e| e == "rs") {
            check_rust_file(root, &path, findings);
        }
    }
}

fn check_rust_file(root: &Path, path: &Path, findings: &mut Vec<Diagnostic>) {
    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return,
    };
    let rel = relative_display(root, path);

    for (line_idx, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with("//!") || trimmed.starts_with("///") {
            for marker in ARCHAEOLOGY_MARKERS {
                if trimmed.contains(marker) {
                    findings.push(framework_finding(
                        RULE_ARCHAEOLOGY,
                        format!(
                            "archaeology marker `{marker}` in doc comment at {rel}:{} — keep ≤3 lines of what-it-does-today; history belongs in DECISIONS.md",
                            line_idx + 1
                        ),
                        Some(loc(path, line_idx + 1, None)),
                    ));
                    break;
                }
            }
        }

        if trimmed.contains("#[allow(") && !trimmed.contains("reason") {
            findings.push(framework_finding(
                RULE_ALLOW_NO_REASON,
                format!(
                    "#[allow] without reason at {rel}:{} — use #[expect] with reason or promote a module #![allow]",
                    line_idx + 1
                ),
                Some(loc(path, line_idx + 1, None)),
            ));
        }
    }
}
