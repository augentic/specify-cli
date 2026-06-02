//! Proposal-brief rules.

use crate::{BriefContext, Classification, Rule, RuleOutcome, primitives};

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
            detail: "`## Units` section missing or has no content".to_string(),
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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{proposal_units_listed, proposal_why_has_content};
    use crate::{BriefContext, RuleOutcome};

    fn ctx(content: &str) -> BriefContext<'_> {
        BriefContext {
            id: "proposal",
            content,
            parsed_spec: None,
            tasks: None,
            slice_dir: Path::new("."),
            specs_dir: Path::new("."),
        }
    }

    #[test]
    fn why_rule_passes_with_prose_and_fails_when_empty() {
        assert_eq!(
            proposal_why_has_content(&ctx("## Why\n\nbecause it matters\n")),
            RuleOutcome::Pass
        );
        assert!(matches!(
            proposal_why_has_content(&ctx("## Why\n\n## Units\n- a\n")),
            RuleOutcome::Fail { .. }
        ));
    }

    #[test]
    fn units_rule_passes_with_entries_and_fails_when_absent() {
        assert_eq!(proposal_units_listed(&ctx("## Units\n\n- login\n")), RuleOutcome::Pass);
        assert!(matches!(
            proposal_units_listed(&ctx("## Why\n\nbecause\n")),
            RuleOutcome::Fail { .. }
        ));
    }
}
