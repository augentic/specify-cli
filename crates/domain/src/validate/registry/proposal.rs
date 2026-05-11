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

fn proposal_crates_listed(ctx: &BriefContext<'_>) -> RuleOutcome {
    let headings: &[&str] = match ctx.terminology {
        "crate" => &["## Crates"],
        "feature" => &["## Features"],
        _ => &["## Crates", "## Features"],
    };
    for heading in headings {
        if primitives::has_content_after_heading(ctx.content, heading) {
            return RuleOutcome::Pass;
        }
    }
    RuleOutcome::Fail {
        detail: format!(
            "deliverables section missing content (looked for {})",
            headings.join(", ")
        ),
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
        id: "proposal.crates-listed",
        description: "Has a Crates/Features section listing at least one entry",
        classification: Classification::Structural,
        check: Some(proposal_crates_listed),
    },
    Rule {
        id: "proposal.uses-imperative-language",
        description: "Uses imperative language for motivation",
        classification: Classification::Semantic,
        check: None,
    },
];
