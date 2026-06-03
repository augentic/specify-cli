//! `kind: authoring-predicate` evaluator (RFC-31 Phase 3 bridge).
//!
//! Runs a closed imperative predicate by authoring `rule_id` against a
//! framework [`Context`] built from `project_dir`. Findings are restamped
//! with the declarative rule's `rule_id` and `severity` so the wire
//! shape matches other hint kinds. Predicates migrate off this bridge as
//! native fact-iterating hints land parity.

use std::path::{Path, PathBuf};

use specify_diagnostics::Diagnostic;

use super::{HintError, restamp_finding as stamp_id};
use crate::framework::check::{
    AgentTeamsCheck, ArgumentHintGrammar, BriefCheck, BrokenSymlinkCheck, Check,
    DeployableLinksCheck, DescriptionGrammar, FirstPartyTools, FrontmatterSchema,
    InlineJsonTooLong, InvalidCriticalPath, LinksCheck, MarketplaceDriftCheck, MissingCriticalPath,
    MissingDiagramAsset, NameDirMismatch, NumericCaps, ScenariosCheck, SchemaLinksCheck,
    SectionLineCount, StepBodyDuplicatesCriticalPath, TextPipelineDiagram, UnknownTool,
    VariableCoverage, run_adapter_check, run_rules_schema_check,
};
use crate::framework::context::Context;
use crate::lint::WorkspaceModel;
use crate::rules::{HintKind, ResolvedRule, RuleHint};

/// Closed authoring `rule_id` values executable via this bridge.
const SUPPORTED: &[&str] = &[
    "adapter.execution-agent",
    "adapter.missing-manifest",
    "agent-teams.missing-canonical",
    "agent-teams.non-canonical-overlay",
    "brief.exceeds-size-limit",
    "docs.missing-diagram-asset",
    "docs.text-pipeline-diagram",
    "links.brief-schema-link-resolve",
    "links.broken-reference",
    "links.docs-in-deployable-surface",
    "links.unresolved-directive",
    "plugins.broken-symlink",
    "plugins.marketplace-drift",
    "prose.numeric-cap-exceeded",
    "rules.duplicate-rule-id",
    "rules.schema-violation",
    "scenarios.artifact-path-unsafe",
    "scenarios.body-id-mismatch",
    "scenarios.duplicate-id",
    "scenarios.recorded-trace-violation",
    "scenarios.schema-violation",
    "scenarios.stages-not-contiguous-prefix",
    "scenarios.stale-recorded-trace",
    "skill.argument-hint-grammar",
    "skill.description-grammar",
    "skill.inline-json-too-long",
    "skill.invalid-critical-path",
    "skill.missing-critical-path",
    "skill.missing-frontmatter",
    "skill.name-directory-mismatch",
    "skill.schema-violation",
    "skill.section-line-count",
    "skill.step-body-duplicates-critical-path",
    "skill.unknown-tool",
    "skill.variable-coverage",
    "tools.invalid-declaration",
];

pub(crate) fn evaluate(
    rule: &ResolvedRule, hint: &RuleHint, _candidates: &[PathBuf], _model: &WorkspaceModel,
    project_dir: &Path, next_id: &mut u64,
) -> Result<Vec<Diagnostic>, HintError> {
    let authoring_id = hint.value.trim();
    if !SUPPORTED.contains(&authoring_id) {
        return Err(HintError::Unsupported {
            rule_id: rule.rule_id.clone(),
            kind: HintKind::AuthoringPredicate,
            reason: "unknown authoring-predicate id",
        });
    }

    let ctx = Context::from_framework_root(project_dir).map_err(|err| HintError::Filesystem {
        op: "framework-root",
        path: project_dir.to_path_buf(),
        source: std::io::Error::other(err.to_string()),
    })?;

    let raw = run_predicate(authoring_id, &ctx);
    let filtered = filter_authoring(raw, authoring_id);
    let mut out = Vec::with_capacity(filtered.len());
    for finding in filtered {
        out.push(remap_for_rule(finding, rule, *next_id));
        *next_id += 1;
    }
    Ok(out)
}

fn remap_for_rule(mut finding: Diagnostic, rule: &ResolvedRule, id_num: u64) -> Diagnostic {
    finding.rule_id = Some(rule.rule_id.clone());
    finding.severity = rule.severity;
    stamp_id(&mut finding, id_num);
    finding
}

fn run_predicate(authoring_id: &str, ctx: &Context) -> Vec<Diagnostic> {
    match authoring_id {
        "adapter.missing-manifest" | "adapter.execution-agent" => run_adapter_check(ctx),
        "agent-teams.missing-canonical" | "agent-teams.non-canonical-overlay" => {
            AgentTeamsCheck.run(ctx)
        }
        "brief.exceeds-size-limit" => BriefCheck.run(ctx),
        "docs.missing-diagram-asset" => MissingDiagramAsset.run(ctx),
        "docs.text-pipeline-diagram" => TextPipelineDiagram.run(ctx),
        "links.broken-reference" | "links.unresolved-directive" => LinksCheck.run(ctx),
        "links.brief-schema-link-resolve" => SchemaLinksCheck.run(ctx),
        "links.docs-in-deployable-surface" => DeployableLinksCheck.run(ctx),
        "plugins.broken-symlink" => BrokenSymlinkCheck.run(ctx),
        "plugins.marketplace-drift" => MarketplaceDriftCheck.run(ctx),
        "prose.numeric-cap-exceeded" => NumericCaps.run(ctx),
        "rules.duplicate-rule-id" | "rules.schema-violation" => run_rules_schema_check(ctx),
        "scenarios.artifact-path-unsafe"
        | "scenarios.body-id-mismatch"
        | "scenarios.duplicate-id"
        | "scenarios.recorded-trace-violation"
        | "scenarios.schema-violation"
        | "scenarios.stages-not-contiguous-prefix"
        | "scenarios.stale-recorded-trace" => ScenariosCheck.run(ctx),
        "skill.missing-frontmatter" | "skill.schema-violation" => FrontmatterSchema.run(ctx),
        "skill.name-directory-mismatch" => NameDirMismatch.run(ctx),
        "skill.unknown-tool" => UnknownTool.run(ctx),
        "skill.description-grammar" => DescriptionGrammar.run(ctx),
        "skill.argument-hint-grammar" => ArgumentHintGrammar.run(ctx),
        "skill.section-line-count" => SectionLineCount.run(ctx),
        "skill.missing-critical-path" => MissingCriticalPath.run(ctx),
        "skill.invalid-critical-path" => InvalidCriticalPath.run(ctx),
        "skill.inline-json-too-long" => InlineJsonTooLong.run(ctx),
        "skill.step-body-duplicates-critical-path" => StepBodyDuplicatesCriticalPath.run(ctx),
        "skill.variable-coverage" => VariableCoverage.run(ctx),
        "tools.invalid-declaration" => FirstPartyTools.run(ctx),
        _ => Vec::new(),
    }
}

fn filter_authoring(findings: Vec<Diagnostic>, authoring_id: &str) -> Vec<Diagnostic> {
    let needle = format!("Authoring check '{authoring_id}' failed.");
    findings.into_iter().filter(|finding| finding.impact == needle).collect()
}
