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

pub use adapter::{AdapterCheck, RULE_MISSING_MANIFEST, RULE_SCHEMA_VIOLATION, run_adapter_check};
pub use agent_teams::AgentTeamsCheck;
pub use brief::BriefCheck;
pub use docs_quality::{MissingDiagramAsset, SpecifyHistoryCitationInDocs, TextPipelineDiagram};
pub use links::LinksCheck;
pub use plugins::{BrokenSymlinkCheck, MarketplaceDriftCheck};
pub use prose::{InvocationPositional, OperationalVocabulary, SkillNumericCaps};
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
    SkillBodyLineCount, SkillEnvelopeJsonInBody, SkillFrontmatterRestatement,
    SkillInlineJsonTooLong, SkillInvalidCriticalPath, SkillMissingCriticalPath,
    SkillSectionLineCount, SkillStepBodyDuplicatesCriticalPath, SkillVariableCoverage,
};
pub use skill_frontmatter::{
    RULE_ARGUMENT_HINT_GRAMMAR, RULE_DESCRIPTION_GRAMMAR, RULE_DUPLICATE_NAME,
    RULE_MISSING_FRONTMATTER, RULE_NAME_DIRECTORY_MISMATCH,
    RULE_SCHEMA_VIOLATION as SKILL_RULE_SCHEMA_VIOLATION, RULE_UNKNOWN_TOOL,
    SkillArgumentHintGrammarCheck, SkillDescriptionGrammarCheck, SkillDuplicateNameCheck,
    SkillFrontmatterSchemaCheck, SkillNameDirectoryMismatchCheck, SkillUnknownToolCheck,
};
pub use tools::{DeclaredToolEquivalentInvocations, FirstPartyToolDeclarations};

use crate::context::Context;
use crate::finding::{Check, Finding};

/// Run every registered check predicate sequentially.
pub fn run(ctx: &Context) -> Vec<Finding> {
    let checks: &[&dyn Check] = &[
        &AdapterCheck,
        &AgentTeamsCheck,
        &BriefCheck,
        &RulesCheck,
        &SpecifyHistoryCitationInDocs,
        &MissingDiagramAsset,
        &TextPipelineDiagram,
        &LinksCheck,
        &BrokenSymlinkCheck,
        &MarketplaceDriftCheck,
        &FirstPartyToolDeclarations,
        &DeclaredToolEquivalentInvocations,
        &OperationalVocabulary,
        &SkillNumericCaps,
        &InvocationPositional,
        &ScenariosCheck,
        &SkillFrontmatterSchemaCheck,
        &SkillNameDirectoryMismatchCheck,
        &SkillDuplicateNameCheck,
        &SkillUnknownToolCheck,
        &SkillDescriptionGrammarCheck,
        &SkillArgumentHintGrammarCheck,
        &SkillBodyLineCount,
        &SkillSectionLineCount,
        &SkillMissingCriticalPath,
        &SkillInvalidCriticalPath,
        &SkillInlineJsonTooLong,
        &SkillEnvelopeJsonInBody,
        &SkillStepBodyDuplicatesCriticalPath,
        &SkillFrontmatterRestatement,
        &SkillVariableCoverage,
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
