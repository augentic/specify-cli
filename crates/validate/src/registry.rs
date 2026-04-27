//! Hardcoded rule registry — the RFC-1a table of representative rules,
//! keyed by brief id, plus the cross-brief rules.
//!
//! Semantic rules declare a `check` function that panics; the runner in
//! [`crate::run`] never invokes those checkers and a test enforces it.

use crate::{BriefContext, Classification, CrossContext, CrossRule, Rule, RuleOutcome, primitives};

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
// Composition
// ---------------------------------------------------------------------------

fn composition_valid_yaml(ctx: &BriefContext<'_>) -> RuleOutcome {
    match serde_yaml_ng::from_str::<serde_yaml_ng::Value>(ctx.content) {
        Ok(_) => RuleOutcome::Pass,
        Err(err) => RuleOutcome::Fail {
            detail: format!("composition.yaml is not valid YAML: {err}"),
        },
    }
}

fn composition_has_version(ctx: &BriefContext<'_>) -> RuleOutcome {
    let doc: serde_yaml_ng::Value = match serde_yaml_ng::from_str(ctx.content) {
        Ok(v) => v,
        Err(_) => {
            return RuleOutcome::Fail {
                detail: "not valid YAML".to_string(),
            };
        }
    };
    match doc.get("version") {
        Some(serde_yaml_ng::Value::Number(n)) if n.as_u64() == Some(1) => RuleOutcome::Pass,
        Some(_) => RuleOutcome::Fail {
            detail: "`version` must be 1".to_string(),
        },
        None => RuleOutcome::Fail {
            detail: "`version` key is missing".to_string(),
        },
    }
}

fn composition_screens_or_delta(ctx: &BriefContext<'_>) -> RuleOutcome {
    let doc: serde_yaml_ng::Value = match serde_yaml_ng::from_str(ctx.content) {
        Ok(v) => v,
        Err(_) => {
            return RuleOutcome::Fail {
                detail: "not valid YAML".to_string(),
            };
        }
    };
    let has_screens = doc.get("screens").is_some();
    let has_delta = doc.get("delta").is_some();
    match (has_screens, has_delta) {
        (true, false) | (false, true) => RuleOutcome::Pass,
        (true, true) => RuleOutcome::Fail {
            detail: "document has both `screens` and `delta` — exactly one must be present"
                .to_string(),
        },
        (false, false) => RuleOutcome::Fail {
            detail: "document has neither `screens` nor `delta` — exactly one must be present"
                .to_string(),
        },
    }
}

fn composition_screen_slugs_kebab(ctx: &BriefContext<'_>) -> RuleOutcome {
    let doc: serde_yaml_ng::Value = match serde_yaml_ng::from_str(ctx.content) {
        Ok(v) => v,
        Err(_) => {
            return RuleOutcome::Fail {
                detail: "not valid YAML".to_string(),
            };
        }
    };
    let slug_re = regex::Regex::new(r"^[a-z][a-z0-9]*(-[a-z0-9]+)*$").unwrap();

    let Some(screens_map) = doc.get("screens").and_then(|s| s.as_mapping()) else {
        if let Some(delta) = doc.get("delta").and_then(|d| d.as_mapping()) {
            let mut bad: Vec<String> = Vec::new();
            for section_key in &["added", "modified", "removed"] {
                if let Some(section) = delta
                    .get(serde_yaml_ng::Value::String(section_key.to_string()))
                    .and_then(|s| s.as_mapping())
                {
                    for key in section.keys() {
                        if let Some(slug) = key.as_str()
                            && !slug_re.is_match(slug)
                        {
                            bad.push(slug.to_string());
                        }
                    }
                }
            }
            if bad.is_empty() {
                return RuleOutcome::Pass;
            }
            return RuleOutcome::Fail {
                detail: format!("non-kebab-case screen slugs in delta: {}", bad.join(", ")),
            };
        }
        return RuleOutcome::Pass;
    };

    let mut bad: Vec<String> = Vec::new();
    for key in screens_map.keys() {
        if let Some(slug) = key.as_str()
            && !slug_re.is_match(slug)
        {
            bad.push(slug.to_string());
        }
    }
    if bad.is_empty() {
        RuleOutcome::Pass
    } else {
        RuleOutcome::Fail {
            detail: format!("non-kebab-case screen slugs: {}", bad.join(", ")),
        }
    }
}

const COMPOSITION_RULES: &[Rule] = &[
    Rule {
        id: "composition.valid-yaml",
        description: "composition.yaml is valid YAML",
        classification: Classification::Structural,
        check: composition_valid_yaml,
    },
    Rule {
        id: "composition.has-version",
        description: "composition.yaml has `version: 1`",
        classification: Classification::Structural,
        check: composition_has_version,
    },
    Rule {
        id: "composition.screens-or-delta",
        description: "Document has exactly one of `screens` or `delta`",
        classification: Classification::Structural,
        check: composition_screens_or_delta,
    },
    Rule {
        id: "composition.screen-slugs-kebab",
        description: "Screen slugs are kebab-case",
        classification: Classification::Structural,
        check: composition_screen_slugs_kebab,
    },
];

// ---------------------------------------------------------------------------
// Contracts
// ---------------------------------------------------------------------------

fn contracts_schemas_dir_has_files(ctx: &BriefContext<'_>) -> RuleOutcome {
    let schemas_dir = ctx.change_dir.join("contracts").join("schemas");
    if !schemas_dir.is_dir() {
        return RuleOutcome::Fail {
            detail: "contracts/schemas/ directory not found in change".to_string(),
        };
    }

    let has_yaml = std::fs::read_dir(&schemas_dir)
        .ok()
        .map(|entries| {
            entries.filter_map(std::result::Result::ok).any(|e| {
                matches!(e.path().extension().and_then(|x| x.to_str()), Some("yaml" | "yml"))
            })
        })
        .unwrap_or(false);

    if has_yaml {
        RuleOutcome::Pass
    } else {
        RuleOutcome::Fail {
            detail: "contracts/schemas/ exists but contains no .yaml files".to_string(),
        }
    }
}

fn contracts_refs_resolve(ctx: &BriefContext<'_>) -> RuleOutcome {
    let contracts_dir = ctx.change_dir.join("contracts");
    let mut failures: Vec<String> = Vec::new();

    for subdir in &["http", "messages"] {
        let dir = contracts_dir.join(subdir);
        if !dir.is_dir() {
            continue;
        }

        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.filter_map(std::result::Result::ok) {
            let path = entry.path();
            if !matches!(path.extension().and_then(|x| x.to_str()), Some("yaml" | "yml")) {
                continue;
            }
            let Ok(content) = std::fs::read_to_string(&path) else {
                continue;
            };
            for line in content.lines() {
                if let Some(ref_value) = primitives::extract_ref(line) {
                    let resolved = dir.join(ref_value);
                    if !resolved.is_file() {
                        failures.push(format!(
                            "{}: $ref '{}' does not resolve",
                            path.file_name().unwrap_or_default().to_string_lossy(),
                            ref_value
                        ));
                    }
                }
            }
        }
    }

    if failures.is_empty() {
        RuleOutcome::Pass
    } else {
        RuleOutcome::Fail {
            detail: failures.join("; "),
        }
    }
}

fn contracts_schema_metadata(ctx: &BriefContext<'_>) -> RuleOutcome {
    let schemas_dir = ctx.change_dir.join("contracts").join("schemas");
    if !schemas_dir.is_dir() {
        return RuleOutcome::Pass;
    }

    let mut failures: Vec<String> = Vec::new();

    let Ok(entries) = std::fs::read_dir(&schemas_dir) else {
        return RuleOutcome::Pass;
    };
    for entry in entries.filter_map(std::result::Result::ok) {
        let path = entry.path();
        if !matches!(path.extension().and_then(|x| x.to_str()), Some("yaml" | "yml")) {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        let filename = path.file_name().unwrap_or_default().to_string_lossy();
        let Ok(doc) = serde_yaml_ng::from_str::<serde_yaml_ng::Value>(&content) else {
            continue;
        };
        if doc.get("$id").is_none() {
            failures.push(format!("{filename}: missing $id"));
        }
        if doc.get("title").and_then(|v| v.as_str()).is_none_or(str::is_empty) {
            failures.push(format!("{filename}: missing or empty title"));
        }
        if doc.get("description").and_then(|v| v.as_str()).is_none_or(str::is_empty) {
            failures.push(format!("{filename}: missing or empty description"));
        }
    }

    if failures.is_empty() {
        RuleOutcome::Pass
    } else {
        RuleOutcome::Fail {
            detail: failures.join("; "),
        }
    }
}

const CONTRACTS_RULES: &[Rule] = &[
    Rule {
        id: "contracts.schemas-dir-has-files",
        description: "contracts/schemas/ directory exists and contains at least one .yaml file",
        classification: Classification::Structural,
        check: contracts_schemas_dir_has_files,
    },
    Rule {
        id: "contracts.refs-resolve",
        description: "$ref pointers in OpenAPI/AsyncAPI files resolve to existing schema files",
        classification: Classification::Structural,
        check: contracts_refs_resolve,
    },
    Rule {
        id: "contracts.schema-metadata",
        description: "JSON Schema files have $id, title, and description fields",
        classification: Classification::Structural,
        check: contracts_schema_metadata,
    },
];

// ---------------------------------------------------------------------------
// Registry lookup
// ---------------------------------------------------------------------------

/// Return the registered rules for `brief_id`. Unknown ids return `&[]`.
#[must_use] 
pub fn rules_for(brief_id: &str) -> &'static [Rule] {
    match brief_id {
        "proposal" => PROPOSAL_RULES,
        "specs" => SPECS_RULES,
        "design" => DESIGN_RULES,
        "tasks" => TASKS_RULES,
        "composition" => COMPOSITION_RULES,
        "contracts" => CONTRACTS_RULES,
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

fn cross_composition_maps_to_consistent(ctx: &CrossContext<'_>) -> RuleOutcome {
    let comp_path = ctx.change_dir.join("composition.yaml");
    let Ok(comp_text) = std::fs::read_to_string(&comp_path) else {
        return RuleOutcome::Pass;
    };

    let doc: serde_yaml_ng::Value = match serde_yaml_ng::from_str(&comp_text) {
        Ok(v) => v,
        Err(_) => return RuleOutcome::Pass,
    };

    let Some(screens) = doc.get("screens").and_then(|s| s.as_mapping()) else {
        if let Some(delta) = doc.get("delta").and_then(|d| d.as_mapping()) {
            let mut maps_to_issues: Vec<String> = Vec::new();
            for section_key in &["added", "modified"] {
                if let Some(section) = delta
                    .get(serde_yaml_ng::Value::String(section_key.to_string()))
                    .and_then(|s| s.as_mapping())
                {
                    for (slug_val, screen) in section {
                        let slug = slug_val.as_str().unwrap_or("?");
                        if let Some(maps_to) = screen.get("maps_to") {
                            if let Some(val) = maps_to.as_str() {
                                if val.is_empty() {
                                    maps_to_issues
                                        .push(format!("screen `{slug}` has empty `maps_to`"));
                                }
                            } else {
                                maps_to_issues
                                    .push(format!("screen `{slug}` has non-string `maps_to`"));
                            }
                        }
                    }
                }
            }
            if maps_to_issues.is_empty() {
                return RuleOutcome::Pass;
            }
            return RuleOutcome::Fail {
                detail: maps_to_issues.join("; "),
            };
        }
        return RuleOutcome::Pass;
    };

    let mut issues: Vec<String> = Vec::new();
    for (slug_val, screen) in screens {
        let slug = slug_val.as_str().unwrap_or("?");
        if let Some(maps_to) = screen.get("maps_to") {
            if let Some(val) = maps_to.as_str() {
                if val.is_empty() {
                    issues.push(format!("screen `{slug}` has empty `maps_to`"));
                }
            } else {
                issues.push(format!("screen `{slug}` has non-string `maps_to`"));
            }
        }
    }

    if issues.is_empty() {
        RuleOutcome::Pass
    } else {
        RuleOutcome::Fail {
            detail: issues.join("; "),
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
    CrossRule {
        id: "cross.composition-maps-to-consistent",
        description: "composition.yaml maps_to values are well-formed",
        classification: Classification::Structural,
        check: cross_composition_maps_to_consistent,
    },
];

#[must_use] 
pub fn cross_rules() -> &'static [CrossRule] {
    CROSS_RULES
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::*;

    fn brief_ctx<'a>(
        change_dir: &'a Path, specs_dir: &'a Path, content: &'a str,
    ) -> BriefContext<'a> {
        BriefContext {
            brief_id: "contracts",
            content,
            parsed_spec: None,
            tasks: None,
            change_dir,
            specs_dir,
            terminology: "crate",
        }
    }

    #[test]
    fn schemas_dir_has_files_passes_with_yaml() {
        let dir = tempfile::tempdir().unwrap();
        let schemas = dir.path().join("contracts").join("schemas");
        fs::create_dir_all(&schemas).unwrap();
        fs::write(schemas.join("user.yaml"), "$id: urn:test\ntitle: U\ndescription: d\n").unwrap();

        let specs_dir = dir.path().join("specs");
        let ctx = brief_ctx(dir.path(), &specs_dir, "");
        assert_eq!(contracts_schemas_dir_has_files(&ctx), RuleOutcome::Pass);
    }

    #[test]
    fn schemas_dir_has_files_fails_with_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let schemas = dir.path().join("contracts").join("schemas");
        fs::create_dir_all(&schemas).unwrap();

        let specs_dir = dir.path().join("specs");
        let ctx = brief_ctx(dir.path(), &specs_dir, "");
        assert!(matches!(contracts_schemas_dir_has_files(&ctx), RuleOutcome::Fail { .. }));
    }

    #[test]
    fn schemas_dir_has_files_fails_with_no_dir() {
        let dir = tempfile::tempdir().unwrap();
        let specs_dir = dir.path().join("specs");
        let ctx = brief_ctx(dir.path(), &specs_dir, "");
        match contracts_schemas_dir_has_files(&ctx) {
            RuleOutcome::Fail { detail } => {
                assert!(detail.contains("not found"), "got: {detail}");
            }
            other => panic!("expected Fail, got {other:?}"),
        }
    }

    #[test]
    fn refs_resolve_passes_with_valid_refs() {
        let dir = tempfile::tempdir().unwrap();
        let contracts = dir.path().join("contracts");
        let http = contracts.join("http");
        let schemas = contracts.join("schemas");
        fs::create_dir_all(&http).unwrap();
        fs::create_dir_all(&schemas).unwrap();
        fs::write(schemas.join("user.yaml"), "title: User\n").unwrap();
        fs::write(
            http.join("api.yaml"),
            "openapi: '3.1.0'\npaths:\n  /users:\n    get:\n      schema:\n        $ref: \"../schemas/user.yaml\"\n",
        ).unwrap();

        let specs_dir = dir.path().join("specs");
        let ctx = brief_ctx(dir.path(), &specs_dir, "");
        assert_eq!(contracts_refs_resolve(&ctx), RuleOutcome::Pass);
    }

    #[test]
    fn refs_resolve_fails_with_broken_ref() {
        let dir = tempfile::tempdir().unwrap();
        let contracts = dir.path().join("contracts");
        let http = contracts.join("http");
        fs::create_dir_all(&http).unwrap();
        fs::write(
            http.join("api.yaml"),
            "openapi: '3.1.0'\npaths:\n  /users:\n    get:\n      schema:\n        $ref: \"../schemas/nonexistent.yaml\"\n",
        ).unwrap();

        let specs_dir = dir.path().join("specs");
        let ctx = brief_ctx(dir.path(), &specs_dir, "");
        match contracts_refs_resolve(&ctx) {
            RuleOutcome::Fail { detail } => {
                assert!(detail.contains("nonexistent.yaml"), "got: {detail}");
            }
            other => panic!("expected Fail, got {other:?}"),
        }
    }

    #[test]
    fn refs_resolve_passes_when_no_http_or_messages_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let specs_dir = dir.path().join("specs");
        let ctx = brief_ctx(dir.path(), &specs_dir, "");
        assert_eq!(contracts_refs_resolve(&ctx), RuleOutcome::Pass);
    }

    #[test]
    fn schema_metadata_passes_with_complete_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let schemas = dir.path().join("contracts").join("schemas");
        fs::create_dir_all(&schemas).unwrap();
        fs::write(
            schemas.join("user.yaml"),
            "$id: urn:specify:schemas/user\ntitle: User\ndescription: A user entity.\ntype: object\n",
        ).unwrap();

        let specs_dir = dir.path().join("specs");
        let ctx = brief_ctx(dir.path(), &specs_dir, "");
        assert_eq!(contracts_schema_metadata(&ctx), RuleOutcome::Pass);
    }

    #[test]
    fn schema_metadata_fails_with_missing_id() {
        let dir = tempfile::tempdir().unwrap();
        let schemas = dir.path().join("contracts").join("schemas");
        fs::create_dir_all(&schemas).unwrap();
        fs::write(
            schemas.join("user.yaml"),
            "title: User\ndescription: A user entity.\ntype: object\n",
        )
        .unwrap();

        let specs_dir = dir.path().join("specs");
        let ctx = brief_ctx(dir.path(), &specs_dir, "");
        match contracts_schema_metadata(&ctx) {
            RuleOutcome::Fail { detail } => {
                assert!(detail.contains("missing $id"), "got: {detail}");
            }
            other => panic!("expected Fail, got {other:?}"),
        }
    }

    #[test]
    fn schema_metadata_fails_with_empty_title() {
        let dir = tempfile::tempdir().unwrap();
        let schemas = dir.path().join("contracts").join("schemas");
        fs::create_dir_all(&schemas).unwrap();
        fs::write(
            schemas.join("user.yaml"),
            "$id: urn:test\ntitle: \"\"\ndescription: A user.\ntype: object\n",
        )
        .unwrap();

        let specs_dir = dir.path().join("specs");
        let ctx = brief_ctx(dir.path(), &specs_dir, "");
        match contracts_schema_metadata(&ctx) {
            RuleOutcome::Fail { detail } => {
                assert!(detail.contains("missing or empty title"), "got: {detail}");
            }
            other => panic!("expected Fail, got {other:?}"),
        }
    }

    #[test]
    fn schema_metadata_passes_when_no_schemas_dir() {
        let dir = tempfile::tempdir().unwrap();
        let specs_dir = dir.path().join("specs");
        let ctx = brief_ctx(dir.path(), &specs_dir, "");
        assert_eq!(contracts_schema_metadata(&ctx), RuleOutcome::Pass);
    }
}
