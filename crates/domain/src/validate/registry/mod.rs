//! Hardcoded rule registry — the table of representative rules keyed
//! by brief id, plus the cross-brief rules. See `DECISIONS.md` §"Change
//! A — Hardcoded rule registry, classification at definition site" for
//! provenance.
//!
//! Semantic rules declare `check: None`; the runner's `if let Some`
//! dispatch in [`crate::validate::run`] materialises them as `Deferred`
//! by construction.

use crate::validate::Rule;

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
