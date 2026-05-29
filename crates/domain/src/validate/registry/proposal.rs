//! Proposal-brief rules.

use crate::validate::{BriefContext, Classification, Rule, RuleOutcome, primitives};

fn proposal_why_has_content(ctx: &BriefContext<'_>) -> RuleOutcome {
    if primitives::has_content_after_heading(ctx.content, "## Why") {
        RuleOutcome::Pass
    } else {
        RuleOutcome::Fail {
            detail: "`## Why` section missing or has no prose".to_string(),
        }
    }
}

fn proposal_units_listed(ctx: &BriefContext<'_>) -> RuleOutcome {
    if primitives::has_content_after_heading(ctx.content, "## Units") {
        RuleOutcome::Pass
    } else {
        RuleOutcome::Fail {
            detail: "`## Units` section missing or has no prose".to_string(),
        }
    }
}

pub(super) const PROPOSAL_RULES: &[Rule] = &[
    Rule {
        id: "proposal.why-has-content",
        description: "Has a Why section with at least one sentence",
        classification: Classification::Structural,
        check: Some(proposal_why_has_content),
    },
    Rule {
        id: "proposal.units-listed",
        description: "Has a Units section listing at least one entry",
        classification: Classification::Structural,
        check: Some(proposal_units_listed),
    },
    Rule {
        id: "proposal.uses-imperative-language",
        description: "Uses imperative language for motivation",
        classification: Classification::Semantic,
        check: None,
    },
];
