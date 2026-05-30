use std::fs;
use std::path::Path;

use serde::Deserialize;
use specify_diagnostics::Diagnostic;

use crate::framework::builder::{framework_finding, loc};
use crate::framework::check::Check;
use crate::framework::context::Context;
use crate::framework::helpers::under_symlink;

pub const RULE_MISSING_MANIFEST: &str = "adapter.missing-manifest";

/// RFC-29 D9 — a first-party adapter declaring `execution: agent`
/// surfaces a `suggestion`-severity finding. The framework `Check`
/// pass only ever runs against the framework repo's own adapter tree,
/// so the finding is scoped to first-party adapters by construction;
/// third-party adapters in consumer projects are scanned by the
/// declarative `specrun lint` pass, which never runs this predicate.
pub const RULE_EXECUTION_AGENT: &str = "adapter.execution-agent";

const ADAPTER_FILENAME: &str = "adapter.yaml";

/// Tolerant probe for the closed `execution:` field. Mirrors the
/// `index/adapter.rs` extractor's "only the field I need" DTO so a
/// malformed body collapses to a silent skip — schema validity is
/// owned by `CORE-001` and the loader, not this suggestion.
#[derive(Debug, Deserialize)]
struct ExecutionProbe {
    execution: Option<String>,
}

/// Adapter directory health predicate.
///
/// Schema validation against `source.schema.json` / `target.schema.json`
/// was retired — `CORE-001` ≅ `adapter.schema` now owns
/// that surface via a `path-pattern` + `schema` deterministic hint
/// pair (`adapters/shared/rules/core/CORE-001-adapter-schema.md` in the
/// framework repo). The parity test
/// `crates/standards/tests/core_parity_adapter_schema.rs` proves the
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
    fn run(&self, ctx: &Context) -> Vec<Diagnostic> {
        run_adapter_check(ctx)
    }
}

pub fn run_adapter_check(ctx: &Context) -> Vec<Diagnostic> {
    let mut findings = Vec::new();
    findings.extend(check_missing_manifests(ctx, &ctx.sources_dir()));
    findings.extend(check_missing_manifests(ctx, &ctx.targets_dir()));
    findings.extend(check_execution_agent(ctx, &ctx.sources_dir()));
    findings.extend(check_execution_agent(ctx, &ctx.targets_dir()));
    findings
}

/// Emit a `suggestion`-severity [`RULE_EXECUTION_AGENT`] finding for
/// every first-party adapter under `axis_dir` whose manifest declares
/// `execution: agent` (RFC-29 D9). Malformed or manifest-less
/// directories are skipped — they are owned by `CORE-001` /
/// [`RULE_MISSING_MANIFEST`].
fn check_execution_agent(ctx: &Context, axis_dir: &Path) -> Vec<Diagnostic> {
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
        let Ok(raw) = fs::read_to_string(&manifest) else {
            continue;
        };
        let Ok(probe) = serde_saphyr::from_str::<ExecutionProbe>(&raw) else {
            continue;
        };
        if probe.execution.as_deref() != Some("agent") {
            continue;
        }
        let rel = relative_path(ctx, &manifest);
        findings.push(framework_finding(
            RULE_EXECUTION_AGENT,
            format!(
                "Adapter declares `execution: agent` (RFC-29 D9): {rel} — the brief runs via an agent and the CLI forces `cache: opt-out`; switch to `execution: tool` once a deterministic dispatch path exists."
            ),
            Some(loc(manifest.clone(), 1, None)),
        ));
    }
    findings.sort_by(|a, b| a.title.cmp(&b.title));
    findings
}

fn check_missing_manifests(ctx: &Context, axis_dir: &Path) -> Vec<Diagnostic> {
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
        findings.push(framework_finding(
            RULE_MISSING_MANIFEST,
            format!("Adapter directory missing manifest: {rel} — expected {ADAPTER_FILENAME}"),
            Some(loc(path.clone(), 1, None)),
        ));
    }
    findings.sort_by(|a, b| a.title.cmp(&b.title));
    findings
}

fn relative_path(ctx: &Context, path: &Path) -> String {
    path.strip_prefix(ctx.framework_root()).unwrap_or(path).display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::framework::builder::{core_id_for, snippet};

    #[test]
    fn relative_path_strips_framework_root() {
        let temp = tempfile::tempdir().expect("tempdir");
        scaffold_framework(temp.path());
        let ctx = Context::from_framework_root(temp.path()).expect("framework root resolves");
        let path = ctx.sources_dir().join("intent").join(ADAPTER_FILENAME);
        assert_eq!(relative_path(&ctx, &path), "adapters/sources/intent/adapter.yaml");
    }

    #[test]
    fn missing_manifest_on_empty_dir() {
        let temp = tempfile::tempdir().expect("tempdir");
        scaffold_framework(temp.path());
        let adapter_dir = temp.path().join("adapters/sources/broken");
        fs::create_dir_all(&adapter_dir).expect("adapter dir");
        let ctx = Context::from_framework_root(temp.path()).expect("context");
        let findings = check_missing_manifests(&ctx, &ctx.sources_dir());
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule_id.as_deref(), core_id_for(RULE_MISSING_MANIFEST));
        assert!(snippet(&findings[0]).contains("adapters/sources/broken"));
    }

    #[test]
    fn execution_agent_emits_suggestion() {
        use specify_diagnostics::Severity;

        let temp = tempfile::tempdir().expect("tempdir");
        scaffold_framework(temp.path());
        let adapter_dir = temp.path().join("adapters/sources/documentation");
        fs::create_dir_all(&adapter_dir).expect("adapter dir");
        fs::write(
            adapter_dir.join(ADAPTER_FILENAME),
            "name: documentation\nversion: 1\naxis: source\nexecution: agent\nbriefs:\n  survey: briefs/survey.md\n  extract: briefs/extract.md\ndescription: Docs source.\n",
        )
        .expect("manifest");
        let ctx = Context::from_framework_root(temp.path()).expect("context");

        let findings = check_execution_agent(&ctx, &ctx.sources_dir());
        assert_eq!(findings.len(), 1, "execution: agent must surface one finding");
        assert_eq!(findings[0].rule_id.as_deref(), core_id_for(RULE_EXECUTION_AGENT));
        assert_eq!(findings[0].severity, Severity::Suggestion, "must not block CI");
        assert!(snippet(&findings[0]).contains("adapters/sources/documentation"));
    }

    #[test]
    fn execution_tool_emits_nothing() {
        let temp = tempfile::tempdir().expect("tempdir");
        scaffold_framework(temp.path());
        let adapter_dir = temp.path().join("adapters/targets/widget");
        fs::create_dir_all(&adapter_dir).expect("adapter dir");
        fs::write(
            adapter_dir.join(ADAPTER_FILENAME),
            "name: widget\nversion: 1\naxis: target\nexecution: tool\nbriefs:\n  shape: briefs/shape.md\n  build: briefs/build.md\n  merge: briefs/merge.md\ndescription: Tool target.\n",
        )
        .expect("manifest");
        let ctx = Context::from_framework_root(temp.path()).expect("context");

        assert!(
            check_execution_agent(&ctx, &ctx.targets_dir()).is_empty(),
            "execution: tool must not surface the agent suggestion"
        );
    }

    fn scaffold_framework(root: &Path) {
        fs::create_dir_all(root.join("plugins")).expect("plugins");
        fs::create_dir_all(root.join("adapters/sources")).expect("sources");
        fs::create_dir_all(root.join("adapters/targets")).expect("targets");
    }
}
