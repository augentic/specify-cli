//! Hardcoded rule registry — the RFC-1a table of representative rules,
//! keyed by brief id, plus the cross-brief rules.
//!
//! Semantic rules declare a `check` function that panics; the runner in
//! [`crate::run`] never invokes those checkers and a test enforces it.

use crate::primitives;
use crate::{BriefContext, Classification, CrossContext, CrossRule, Rule, RuleOutcome};

// ---------------------------------------------------------------------------
// Proposal
// ---------------------------------------------------------------------------

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

fn semantic_never_called(_ctx: &BriefContext<'_>) -> RuleOutcome {
    panic!("semantic rule checker should never be invoked");
}

const PROPOSAL_RULES: &[Rule] = &[
    Rule {
        id: "proposal.why-has-content",
        description: "Has a Why section with at least one sentence",
        classification: Classification::Structural,
        check: proposal_why_has_content,
    },
    Rule {
        id: "proposal.crates-listed",
        description: "Has a Crates/Features section listing at least one entry",
        classification: Classification::Structural,
        check: proposal_crates_listed,
    },
    Rule {
        id: "proposal.uses-imperative-language",
        description: "Uses imperative language for motivation",
        classification: Classification::Semantic,
        check: semantic_never_called,
    },
];

// ---------------------------------------------------------------------------
// Specs
// ---------------------------------------------------------------------------

fn specs_requirements_have_scenarios(ctx: &BriefContext<'_>) -> RuleOutcome {
    let Some(spec) = ctx.parsed_spec else {
        return RuleOutcome::Fail {
            detail: "spec was not parsed".to_string(),
        };
    };
    if primitives::all_requirements_have_scenarios(spec) {
        RuleOutcome::Pass
    } else {
        RuleOutcome::Fail {
            detail: "one or more requirements have no scenarios".to_string(),
        }
    }
}

fn specs_requirements_have_ids(ctx: &BriefContext<'_>) -> RuleOutcome {
    let Some(spec) = ctx.parsed_spec else {
        return RuleOutcome::Fail {
            detail: "spec was not parsed".to_string(),
        };
    };
    if primitives::all_requirements_have_ids(spec) {
        RuleOutcome::Pass
    } else {
        RuleOutcome::Fail {
            detail: "one or more requirements are missing an ID".to_string(),
        }
    }
}

fn specs_ids_match_pattern(ctx: &BriefContext<'_>) -> RuleOutcome {
    let Some(spec) = ctx.parsed_spec else {
        return RuleOutcome::Fail {
            detail: "spec was not parsed".to_string(),
        };
    };
    if primitives::ids_match_pattern(spec, specify_spec::REQUIREMENT_ID_PATTERN) {
        RuleOutcome::Pass
    } else {
        RuleOutcome::Fail {
            detail: format!(
                "one or more requirement IDs do not match `{}`",
                specify_spec::REQUIREMENT_ID_PATTERN
            ),
        }
    }
}

const SPECS_RULES: &[Rule] = &[
    Rule {
        id: "specs.requirements-have-scenarios",
        description: "Every requirement has at least one scenario",
        classification: Classification::Structural,
        check: specs_requirements_have_scenarios,
    },
    Rule {
        id: "specs.requirements-have-ids",
        description: "Every requirement has an `ID:` line",
        classification: Classification::Structural,
        check: specs_requirements_have_ids,
    },
    Rule {
        id: "specs.ids-match-pattern",
        description: "IDs use the `REQ-[0-9]{3}` format",
        classification: Classification::Structural,
        check: specs_ids_match_pattern,
    },
    Rule {
        id: "specs.uses-normative-language",
        description: "Uses SHALL/MUST language for normative requirements",
        classification: Classification::Semantic,
        check: semantic_never_called,
    },
];

// ---------------------------------------------------------------------------
// Design
// ---------------------------------------------------------------------------

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

const DESIGN_RULES: &[Rule] = &[Rule {
    id: "design.references-valid-ids",
    description: "References only requirement ids present in specs",
    classification: Classification::Structural,
    check: design_references_valid_ids,
}];

// ---------------------------------------------------------------------------
// Tasks
// ---------------------------------------------------------------------------

fn tasks_use_checkbox_format(ctx: &BriefContext<'_>) -> RuleOutcome {
    let Some(tasks) = ctx.tasks else {
        return RuleOutcome::Fail {
            detail: "tasks were not parsed".to_string(),
        };
    };
    if primitives::all_tasks_use_checkbox(tasks, ctx.content) {
        RuleOutcome::Pass
    } else {
        RuleOutcome::Fail {
            detail: "found `- …` bullets that do not match the `- [ ] X.Y` checkbox format"
                .to_string(),
        }
    }
}

fn tasks_grouped_under_headings(ctx: &BriefContext<'_>) -> RuleOutcome {
    let Some(tasks) = ctx.tasks else {
        return RuleOutcome::Fail {
            detail: "tasks were not parsed".to_string(),
        };
    };
    if primitives::tasks_grouped_under_headings(tasks) {
        RuleOutcome::Pass
    } else {
        RuleOutcome::Fail {
            detail: "one or more tasks appear before any `## ` heading".to_string(),
        }
    }
}

const TASKS_RULES: &[Rule] = &[
    Rule {
        id: "tasks.use-checkbox-format",
        description: "All tasks use `- [ ] X.Y` checkbox format",
        classification: Classification::Structural,
        check: tasks_use_checkbox_format,
    },
    Rule {
        id: "tasks.grouped-under-headings",
        description: "Tasks grouped under `## ` headings",
        classification: Classification::Structural,
        check: tasks_grouped_under_headings,
    },
];

// ---------------------------------------------------------------------------
// Registry lookup
// ---------------------------------------------------------------------------

/// Return the registered rules for `brief_id`. Unknown ids return `&[]`.
pub fn rules_for(brief_id: &str) -> &'static [Rule] {
    match brief_id {
        "proposal" => PROPOSAL_RULES,
        "specs" => SPECS_RULES,
        "design" => DESIGN_RULES,
        "tasks" => TASKS_RULES,
        _ => &[],
    }
}

// ---------------------------------------------------------------------------
// Cross-rules
// ---------------------------------------------------------------------------

fn cross_proposal_crates_have_specs(ctx: &CrossContext<'_>) -> RuleOutcome {
    // Locate the proposal artifact via the PipelineView.
    let Some(proposal_brief) = ctx.pipeline.brief("proposal") else {
        // No proposal brief in the pipeline → nothing to check.
        return RuleOutcome::Pass;
    };
    let Some(generates) = proposal_brief.frontmatter.generates.as_deref() else {
        return RuleOutcome::Pass;
    };
    let proposal_path = ctx.change_dir.join(generates);
    let proposal_text = match std::fs::read_to_string(&proposal_path) {
        Ok(t) => t,
        Err(err) => {
            return RuleOutcome::Fail {
                detail: format!("failed to read proposal `{}`: {err}", proposal_path.display()),
            };
        }
    };
    if primitives::proposal_deliverables_have_specs(&proposal_text, ctx.specs_dir, ctx.terminology)
    {
        RuleOutcome::Pass
    } else {
        RuleOutcome::Fail {
            detail: "one or more crates/features listed in the proposal have no matching spec file"
                .to_string(),
        }
    }
}

fn cross_design_references_valid(ctx: &CrossContext<'_>) -> RuleOutcome {
    let Some(design_brief) = ctx.pipeline.brief("design") else {
        return RuleOutcome::Pass;
    };
    let Some(generates) = design_brief.frontmatter.generates.as_deref() else {
        return RuleOutcome::Pass;
    };
    let design_path = ctx.change_dir.join(generates);
    let design_text = match std::fs::read_to_string(&design_path) {
        Ok(t) => t,
        Err(err) => {
            return RuleOutcome::Fail {
                detail: format!("failed to read design `{}`: {err}", design_path.display()),
            };
        }
    };
    if primitives::design_references_exist(&design_text, ctx.specs_dir) {
        RuleOutcome::Pass
    } else {
        RuleOutcome::Fail {
            detail: "design.md references requirement IDs that are not present in the baseline"
                .to_string(),
        }
    }
}

const CROSS_RULES: &[CrossRule] = &[
    CrossRule {
        id: "cross.proposal-crates-have-specs",
        description: "Every crate/feature listed in the proposal has a matching spec file",
        classification: Classification::Structural,
        check: cross_proposal_crates_have_specs,
    },
    CrossRule {
        id: "cross.design-references-valid",
        description: "Every requirement id referenced in design.md exists in specs",
        classification: Classification::Structural,
        check: cross_design_references_valid,
    },
];

pub fn cross_rules() -> &'static [CrossRule] {
    CROSS_RULES
}
