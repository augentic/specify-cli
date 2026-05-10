//! Hardcoded rule registry — the table of representative rules keyed
//! by brief id, plus the cross-brief rules. See `DECISIONS.md` §"Change
//! A — Hardcoded rule registry, classification at definition site" for
//! provenance.
//!
//! Semantic rules declare a `check` function that panics; the runner in
//! [`crate::run`] never invokes those checkers and a test enforces it.

use crate::{BriefContext, Rule, RuleOutcome};

mod composition;
mod contract_rules;
mod cross;
mod design;
mod proposal;
mod specs;
mod tasks;

pub use cross::cross_rules;

/// Return the registered rules for `brief_id`. Unknown ids return `&[]`.
#[must_use]
pub fn rules_for(brief_id: &str) -> &'static [Rule] {
    match brief_id {
        "proposal" => proposal::PROPOSAL_RULES,
        "specs" => specs::SPECS_RULES,
        "design" => design::DESIGN_RULES,
        "tasks" => tasks::TASKS_RULES,
        "composition" => composition::COMPOSITION_RULES,
        "contracts" => contract_rules::CONTRACTS_RULES,
        _ => &[],
    }
}

/// Stub used by every semantic rule. The runner never calls this; the
/// panic exists as a tripwire to catch a future refactor that would.
pub fn semantic_never_called(_ctx: &BriefContext<'_>) -> RuleOutcome {
    panic!("semantic rule checker should never be invoked");
}
