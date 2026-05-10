//! Design-brief rules.

use crate::{BriefContext, Classification, Rule, RuleOutcome, primitives};

fn design_references_valid_ids(ctx: &BriefContext<'_>) -> RuleOutcome {
    if primitives::design_references_exist(ctx.content, ctx.specs_dir) {
        RuleOutcome::Pass
    } else {
        RuleOutcome::Fail {
            detail: "design.md references requirement IDs not present in any baseline spec"
                .to_string(),
        }
    }
}

pub(super) const DESIGN_RULES: &[Rule] = &[Rule {
    id: "design.references-valid-ids",
    description: "References only requirement ids present in specs",
    classification: Classification::Structural,
    check: design_references_valid_ids,
}];
