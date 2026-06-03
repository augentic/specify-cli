pub mod adapter;
pub mod agent_teams;
pub mod brief;
mod deployable_links;
mod docs_quality;
pub mod links;
mod plugins;
mod prose;
pub mod rules;
mod rust_source;
mod rust_test_naming;
pub mod scenarios;
pub mod schema_alias;
pub mod schema_links;
mod skill_body;
pub mod skill_frontmatter;
pub mod tools;

use std::path::Path;

pub use adapter::{AdapterCheck, RULE_EXECUTION_AGENT, RULE_MISSING_MANIFEST, run_adapter_check};
pub use agent_teams::AgentTeamsCheck;
pub use brief::BriefCheck;
pub use deployable_links::DeployableLinksCheck;
pub use docs_quality::{HistoryCitation, MissingDiagramAsset, TextPipelineDiagram};
pub use links::LinksCheck;
pub use plugins::{BrokenSymlinkCheck, MarketplaceDriftCheck};
pub use prose::{InvocationPositional, NumericCaps, OperationalVocabulary};
pub use rules::{
    RULE_DUPLICATE_RULE_ID, RULE_NAMESPACE_OWNERSHIP_VIOLATION, RulesCheck, run_rules_check,
};
pub use rust_source::RustSourceQuality;
pub use rust_test_naming::RustTestNaming;
pub use scenarios::{
    RULE_ARTIFACT_PATH_UNSAFE as SCENARIO_RULE_ARTIFACT_PATH_UNSAFE,
    RULE_BODY_ID_MISMATCH as SCENARIO_RULE_BODY_ID_MISMATCH,
    RULE_DUPLICATE_ID as SCENARIO_RULE_DUPLICATE_ID, RULE_RECORDED_TRACE_VIOLATION,
    RULE_SCHEMA_VIOLATION as SCENARIO_RULE_SCHEMA_VIOLATION, RULE_STAGES_NOT_CONTIGUOUS,
    RULE_STALE_RECORDED_TRACE, ScenariosCheck, check_recorded_trace_freshness,
    validate_scenario_frontmatter,
};
pub use schema_alias::SchemaAliasCheck;
pub use schema_links::SchemaLinksCheck;
pub use skill_body::{
    EnvelopeJsonInBody, FrontmatterRestatement, InlineJsonTooLong, InvalidCriticalPath,
    MissingCriticalPath, SectionLineCount, StepBodyDuplicatesCriticalPath, VariableCoverage,
};
pub use skill_frontmatter::{
    ArgumentHintGrammar, DescriptionGrammar, FrontmatterSchema, NameDirMismatch,
    RULE_ARGUMENT_HINT_GRAMMAR, RULE_DESCRIPTION_GRAMMAR, RULE_MISSING_FRONTMATTER,
    RULE_NAME_DIRECTORY_MISMATCH, RULE_SCHEMA_VIOLATION as SKILL_RULE_SCHEMA_VIOLATION,
    RULE_UNKNOWN_TOOL, UnknownTool,
};
use specify_diagnostics::{Diagnostic, fingerprint};
pub use tools::{DeclaredToolInvocations, FirstPartyTools};

use crate::framework::context::Context;

/// A check predicate that scans the framework repo and returns
/// [`Diagnostic`]s. The predicates need a `&Context` (framework root +
/// schema cache), which the declarative
/// [`crate::lint::producer::DiagnosticProducer`] contract does not
/// provide, so this trait survives the finding-type unification — only
/// its return type changed from the deleted lightweight `Finding`.
pub trait Check {
    /// Scan `ctx` and return this predicate's findings. Locations are
    /// absolute (anchored at the canonicalised framework root) and
    /// `id` / `fingerprint` are left unset for [`run`] to finalise.
    fn run(&self, ctx: &Context) -> Vec<Diagnostic>;
}

/// Rust-quality predicates for the specify-cli repo (`RustTestNaming`,
/// `RustSourceQuality`). No-op on plugin framework roots.
pub fn run_rust_quality(ctx: &Context) -> Vec<Diagnostic> {
    let checks: [&dyn Check; 2] = [&RustTestNaming, &RustSourceQuality];
    let mut findings = Vec::new();
    for check in checks {
        findings.extend(check.run(ctx));
    }
    finalize(&mut findings, ctx.framework_root());
    findings
}

/// Run every registered check predicate sequentially, then finalise the
/// combined batch.
pub fn run(ctx: &Context) -> Vec<Diagnostic> {
    let checks: &[&dyn Check] = &[
        &AdapterCheck,
        &AgentTeamsCheck,
        &BriefCheck,
        &RulesCheck,
        &HistoryCitation,
        &RustTestNaming,
        &RustSourceQuality,
        &MissingDiagramAsset,
        &TextPipelineDiagram,
        &LinksCheck,
        &DeployableLinksCheck,
        &BrokenSymlinkCheck,
        &MarketplaceDriftCheck,
        &FirstPartyTools,
        &DeclaredToolInvocations,
        &OperationalVocabulary,
        &NumericCaps,
        &InvocationPositional,
        &ScenariosCheck,
        &FrontmatterSchema,
        &NameDirMismatch,
        &UnknownTool,
        &DescriptionGrammar,
        &ArgumentHintGrammar,
        &SectionLineCount,
        &MissingCriticalPath,
        &InvalidCriticalPath,
        &InlineJsonTooLong,
        &EnvelopeJsonInBody,
        &StepBodyDuplicatesCriticalPath,
        &FrontmatterRestatement,
        &VariableCoverage,
        &SchemaLinksCheck,
        &SchemaAliasCheck,
    ];
    let mut findings = Vec::new();

    for check in checks {
        findings.extend(check.run(ctx));
    }

    finalize(&mut findings, ctx.framework_root());
    findings
}

/// Finalise a batch of predicate findings into ready-to-render
/// [`Diagnostic`]s: rebase each `location.path` to project-relative
/// form, sort deterministically, then compute fingerprints and assign
/// sequential `FIND-NNNN` ids.
///
/// The fingerprint preimage excludes `id`, so hashing before assigning
/// ids is safe. Rebasing before hashing is required because the
/// imperative predicates emit absolute paths anchored at the
/// canonicalised framework root, while `diagnostic.schema.json`
/// constrains `location.path` to project-relative forward-slash
/// strings.
fn finalize(findings: &mut [Diagnostic], framework_root: &Path) {
    let prefix = framework_root.to_string_lossy().replace('\\', "/");
    for finding in findings.iter_mut() {
        if let Some(location) = finding.location.as_mut() {
            let normalised = location.path.replace('\\', "/");
            if let Some(rest) = normalised.strip_prefix(&prefix) {
                location.path = rest.trim_start_matches('/').to_string();
            } else {
                location.path = normalised;
            }
        }
    }

    findings.sort_by(|a, b| {
        let a_path = a.location.as_ref().map(|l| l.path.as_str());
        let b_path = b.location.as_ref().map(|l| l.path.as_str());
        let a_line = a.location.as_ref().and_then(|l| l.line);
        let b_line = b.location.as_ref().and_then(|l| l.line);
        (&a.rule_id, a_path, a_line, &a.title).cmp(&(&b.rule_id, b_path, b_line, &b.title))
    });

    for (index, finding) in findings.iter_mut().enumerate() {
        finding.fingerprint = fingerprint(finding);
        finding.id = format!("FIND-{:04}", index + 1);
    }
}
