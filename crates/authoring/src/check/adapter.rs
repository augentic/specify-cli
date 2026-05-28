use std::fs;
use std::path::Path;

use crate::context::Context;
use crate::finding::{Check, Finding, Location};
use crate::helpers::under_symlink;

pub const RULE_MISSING_MANIFEST: &str = "adapter.missing-manifest";
const ADAPTER_FILENAME: &str = "adapter.yaml";

/// Adapter directory health predicate.
///
/// Schema validation against `source.schema.json` / `target.schema.json`
/// was retired in RFC-34 C8 — `CORE-001` ≅ `adapter.schema` now owns
/// that surface via a `path-pattern` + `schema` deterministic hint
/// pair (`adapters/shared/rules/core/CORE-001-adapter-schema.md` in the
/// framework repo). The parity test
/// `crates/authoring/tests/core_parity_adapter_schema.rs` proves the
/// declarative pipeline cites the same `iter_errors` instance pointers
/// as the deleted imperative row, with the documented rule-id mapping
/// `adapter.schema-violation` ↔ `CORE-001`. The missing-manifest check
/// below stays imperative because the `set-coverage` reserved-kind
/// hint (C12) iterates `WorkspaceModel.adapter_manifests` and only
/// fires on present-but-incomplete manifests; an axis directory
/// missing its `adapter.yaml` produces no manifest fact and is
/// therefore invisible to the declarative pass. The closer fit for
/// directory existence is a future `cardinality` / `set-eq` rule.
pub struct AdapterCheck;

impl Check for AdapterCheck {
    fn run(&self, ctx: &Context) -> Vec<Finding> {
        run_adapter_check(ctx)
    }
}

pub fn run_adapter_check(ctx: &Context) -> Vec<Finding> {
    let mut findings = Vec::new();
    findings.extend(check_missing_manifests(ctx, &ctx.sources_dir()));
    findings.extend(check_missing_manifests(ctx, &ctx.targets_dir()));
    findings
}

fn check_missing_manifests(ctx: &Context, axis_dir: &Path) -> Vec<Finding> {
    let Ok(entries) = fs::read_dir(axis_dir) else {
        return Vec::new();
    };

    let mut findings = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if under_symlink(ctx.framework_root(), &path).unwrap_or(true) {
            continue;
        }
        let manifest = path.join(ADAPTER_FILENAME);
        if manifest.is_file() {
            continue;
        }
        let rel = relative_path(ctx, &path);
        findings.push(Finding {
            rule_id: RULE_MISSING_MANIFEST,
            message: format!(
                "Adapter directory missing manifest: {rel} — expected {ADAPTER_FILENAME}"
            ),
            location: Some(Location {
                path: path.clone(),
                line: 1,
                column: None,
            }),
        });
    }
    findings.sort_by(|a, b| a.message.cmp(&b.message));
    findings
}

fn relative_path(ctx: &Context, path: &Path) -> String {
    path.strip_prefix(ctx.framework_root()).unwrap_or(path).display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_path_strips_framework_root() {
        let temp = tempfile::tempdir().expect("tempdir");
        scaffold_framework(temp.path());
        let ctx = Context::from_framework_root(temp.path()).expect("framework root resolves");
        let path = ctx.sources_dir().join("intent").join(ADAPTER_FILENAME);
        assert_eq!(relative_path(&ctx, &path), "adapters/sources/intent/adapter.yaml");
    }

    #[test]
    fn missing_manifest_detects_empty_adapter_directory() {
        let temp = tempfile::tempdir().expect("tempdir");
        scaffold_framework(temp.path());
        let adapter_dir = temp.path().join("adapters/sources/broken");
        fs::create_dir_all(&adapter_dir).expect("adapter dir");
        let ctx = Context::from_framework_root(temp.path()).expect("context");
        let findings = check_missing_manifests(&ctx, &ctx.sources_dir());
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule_id, RULE_MISSING_MANIFEST);
        assert!(findings[0].message.contains("adapters/sources/broken"));
    }

    fn scaffold_framework(root: &Path) {
        fs::create_dir_all(root.join("plugins")).expect("plugins");
        fs::create_dir_all(root.join("adapters/sources")).expect("sources");
        fs::create_dir_all(root.join("adapters/targets")).expect("targets");
    }
}
