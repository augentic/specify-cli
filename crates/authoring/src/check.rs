pub mod adapter;
pub mod agent_teams;
pub mod brief;
mod docs_quality;
pub mod links;
mod plugins;
mod prose;
pub mod rules;
pub mod scenarios;
pub mod schema_links;
mod skill_body;
pub mod skill_frontmatter;
pub mod tools;

pub use adapter::{AdapterCheck, RULE_MISSING_MANIFEST, run_adapter_check};
pub use agent_teams::AgentTeamsCheck;
pub use brief::BriefCheck;
pub use docs_quality::{HistoryCitation, MissingDiagramAsset, TextPipelineDiagram};
pub use links::LinksCheck;
pub use plugins::{BrokenSymlinkCheck, MarketplaceDriftCheck};
pub use prose::{InvocationPositional, NumericCaps, OperationalVocabulary};
pub use rules::{
    RULE_DUPLICATE_RULE_ID, RULE_NAMESPACE_OWNERSHIP_VIOLATION, RulesCheck, run_rules_check,
};
pub use scenarios::{
    RULE_ARTIFACT_PATH_UNSAFE as SCENARIO_RULE_ARTIFACT_PATH_UNSAFE,
    RULE_BODY_ID_MISMATCH as SCENARIO_RULE_BODY_ID_MISMATCH,
    RULE_DUPLICATE_ID as SCENARIO_RULE_DUPLICATE_ID, RULE_RECORDED_TRACE_VIOLATION,
    RULE_SCHEMA_VIOLATION as SCENARIO_RULE_SCHEMA_VIOLATION, RULE_STAGES_NOT_CONTIGUOUS,
    RULE_STALE_RECORDED_TRACE, ScenariosCheck, check_recorded_trace_freshness,
    validate_scenario_frontmatter,
};
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
pub use tools::{DeclaredToolInvocations, FirstPartyTools};

use crate::context::Context;
use crate::finding::{Check, Finding};

/// Run every registered check predicate sequentially.
pub fn run(ctx: &Context) -> Vec<Finding> {
    let checks: &[&dyn Check] = &[
        &AdapterCheck,
        &AgentTeamsCheck,
        &BriefCheck,
        &RulesCheck,
        &HistoryCitation,
        &MissingDiagramAsset,
        &TextPipelineDiagram,
        &LinksCheck,
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
    ];
    let mut findings = Vec::new();

    for check in checks {
        findings.extend(check.run(ctx));
    }

    findings.sort_by(|a, b| {
        let a_path = a.location.as_ref().map(|l| l.path.as_path());
        let b_path = b.location.as_ref().map(|l| l.path.as_path());
        let a_line = a.location.as_ref().map(|l| l.line);
        let b_line = b.location.as_ref().map(|l| l.line);
        (a.rule_id, a_path, a_line, &a.message).cmp(&(b.rule_id, b_path, b_line, &b.message))
    });

    findings
}
