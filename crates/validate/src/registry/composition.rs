//! Composition-brief rules.

use crate::{BriefContext, Classification, Rule, RuleOutcome};

fn composition_valid_yaml(ctx: &BriefContext<'_>) -> RuleOutcome {
    match serde_saphyr::from_str::<serde_json::Value>(ctx.content) {
        Ok(_) => RuleOutcome::Pass,
        Err(err) => RuleOutcome::Fail {
            detail: format!("composition.yaml is not valid YAML: {err}"),
        },
    }
}

fn composition_has_version(ctx: &BriefContext<'_>) -> RuleOutcome {
    let doc: serde_json::Value = match serde_saphyr::from_str(ctx.content) {
        Ok(v) => v,
        Err(_) => {
            return RuleOutcome::Fail {
                detail: "not valid YAML".to_string(),
            };
        }
    };
    match doc.get("version") {
        Some(serde_json::Value::Number(n)) if n.as_u64() == Some(1) => RuleOutcome::Pass,
        Some(_) => RuleOutcome::Fail {
            detail: "`version` must be 1".to_string(),
        },
        None => RuleOutcome::Fail {
            detail: "`version` key is missing".to_string(),
        },
    }
}

fn composition_screens_or_delta(ctx: &BriefContext<'_>) -> RuleOutcome {
    let doc: serde_json::Value = match serde_saphyr::from_str(ctx.content) {
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
    let doc: serde_json::Value = match serde_saphyr::from_str(ctx.content) {
        Ok(v) => v,
        Err(_) => {
            return RuleOutcome::Fail {
                detail: "not valid YAML".to_string(),
            };
        }
    };
    let slug_re = regex::Regex::new(r"^[a-z][a-z0-9]*(-[a-z0-9]+)*$").unwrap();

    let Some(screens_map) = doc.get("screens").and_then(|s| s.as_object()) else {
        if let Some(delta) = doc.get("delta").and_then(|d| d.as_object()) {
            let mut bad: Vec<String> = Vec::new();
            for section_key in &["added", "modified", "removed"] {
                if let Some(section) = delta.get(*section_key).and_then(|s| s.as_object()) {
                    for slug in section.keys() {
                        if !slug_re.is_match(slug) {
                            bad.push(slug.clone());
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
    for slug in screens_map.keys() {
        if !slug_re.is_match(slug) {
            bad.push(slug.clone());
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

pub(super) const COMPOSITION_RULES: &[Rule] = &[
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
