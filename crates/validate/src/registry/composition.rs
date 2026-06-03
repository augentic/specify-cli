//! Composition-brief rules.

use regex::Regex;

use crate::{BriefContext, Classification, Rule, RuleOutcome, primitives};

fn parse_composition(ctx: &BriefContext<'_>) -> Result<serde_json::Value, RuleOutcome> {
    match serde_saphyr::from_str(ctx.content) {
        Ok(v) => Ok(v),
        Err(_err) => Err(RuleOutcome::Fail {
            detail: "not valid YAML".to_string(),
        }),
    }
}

fn slug_re() -> &'static Regex {
    primitives::screen_slug_re()
}

fn composition_valid_yaml(ctx: &BriefContext<'_>) -> RuleOutcome {
    match parse_composition(ctx) {
        Ok(_) => RuleOutcome::Pass,
        Err(outcome) => outcome,
    }
}

fn composition_has_version(ctx: &BriefContext<'_>) -> RuleOutcome {
    let doc = match parse_composition(ctx) {
        Ok(v) => v,
        Err(outcome) => return outcome,
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
    let doc = match parse_composition(ctx) {
        Ok(v) => v,
        Err(outcome) => return outcome,
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
    let doc = match parse_composition(ctx) {
        Ok(v) => v,
        Err(outcome) => return outcome,
    };

    let Some(screens_map) = doc.get("screens").and_then(|s| s.as_object()) else {
        if let Some(delta) = doc.get("delta").and_then(|d| d.as_object()) {
            let mut bad: Vec<String> = Vec::new();
            for section_key in &["added", "modified", "removed"] {
                if let Some(section) = delta.get(*section_key).and_then(|s| s.as_object()) {
                    for slug in section.keys() {
                        if !slug_re().is_match(slug) {
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
        if !slug_re().is_match(slug) {
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
        check: Some(composition_valid_yaml),
    },
    Rule {
        id: "composition.has-version",
        description: "composition.yaml has `version: 1`",
        classification: Classification::Structural,
        check: Some(composition_has_version),
    },
    Rule {
        id: "composition.screens-or-delta",
        description: "Document has exactly one of `screens` or `delta`",
        classification: Classification::Structural,
        check: Some(composition_screens_or_delta),
    },
    Rule {
        id: "composition.screen-slugs-kebab",
        description: "Screen slugs are kebab-case",
        classification: Classification::Structural,
        check: Some(composition_screen_slugs_kebab),
    },
];

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{
        composition_has_version, composition_screen_slugs_kebab, composition_screens_or_delta,
        composition_valid_yaml,
    };
    use crate::BriefContext;

    fn ctx(content: &str) -> BriefContext<'_> {
        BriefContext {
            id: "composition",
            content,
            parsed_spec: None,
            tasks: None,
            slice_dir: Path::new("."),
            specs_dir: Path::new("."),
        }
    }

    mod valid_yaml {
        use super::{composition_valid_yaml, ctx};
        use crate::RuleOutcome;

        #[test]
        fn passes_on_well_formed() {
            assert_eq!(composition_valid_yaml(&ctx("version: 1\n")), RuleOutcome::Pass);
        }

        #[test]
        fn fails_on_garbage() {
            // Unterminated flow sequence — serde_saphyr rejects it.
            assert!(matches!(
                composition_valid_yaml(&ctx("screens: [unterminated\n")),
                RuleOutcome::Fail { detail } if detail.contains("not valid YAML")
            ));
        }
    }

    mod has_version {
        use super::{composition_has_version, ctx};
        use crate::RuleOutcome;

        #[test]
        fn passes_on_one() {
            assert_eq!(composition_has_version(&ctx("version: 1\n")), RuleOutcome::Pass);
        }

        #[test]
        fn rejects_other_number() {
            assert!(matches!(
                composition_has_version(&ctx("version: 2\n")),
                RuleOutcome::Fail { detail } if detail.contains("must be 1")
            ));
        }

        #[test]
        fn rejects_missing() {
            assert!(matches!(
                composition_has_version(&ctx("screens: {}\n")),
                RuleOutcome::Fail { detail } if detail.contains("missing")
            ));
        }
    }

    mod screens_or_delta {
        use super::{composition_screens_or_delta, ctx};
        use crate::RuleOutcome;

        #[test]
        fn passes_with_screens_only() {
            assert_eq!(
                composition_screens_or_delta(&ctx("version: 1\nscreens:\n  home: {}\n")),
                RuleOutcome::Pass
            );
        }

        #[test]
        fn passes_with_delta_only() {
            assert_eq!(
                composition_screens_or_delta(&ctx("version: 1\ndelta:\n  added: {}\n")),
                RuleOutcome::Pass
            );
        }

        #[test]
        fn fails_when_both_present() {
            assert!(matches!(
                composition_screens_or_delta(&ctx("screens: {}\ndelta: {}\n")),
                RuleOutcome::Fail { detail } if detail.contains("both")
            ));
        }

        #[test]
        fn fails_when_neither_present() {
            assert!(matches!(
                composition_screens_or_delta(&ctx("version: 1\n")),
                RuleOutcome::Fail { detail } if detail.contains("neither")
            ));
        }
    }

    mod screen_slugs {
        use super::{composition_screen_slugs_kebab, ctx};
        use crate::RuleOutcome;

        #[test]
        fn passes_on_kebab_screens() {
            assert_eq!(
                composition_screen_slugs_kebab(&ctx("screens:\n  user-profile: {}\n")),
                RuleOutcome::Pass
            );
        }

        #[test]
        fn fails_on_non_kebab_screen() {
            assert!(matches!(
                composition_screen_slugs_kebab(&ctx("screens:\n  User_Profile: {}\n")),
                RuleOutcome::Fail { detail } if detail.contains("User_Profile")
            ));
        }

        #[test]
        fn fails_on_non_kebab_delta_slug() {
            assert!(matches!(
                composition_screen_slugs_kebab(&ctx("delta:\n  added:\n    Bad_Slug: {}\n")),
                RuleOutcome::Fail { detail } if detail.contains("Bad_Slug")
            ));
        }

        #[test]
        fn passes_when_neither_screens_nor_delta() {
            assert_eq!(composition_screen_slugs_kebab(&ctx("version: 1\n")), RuleOutcome::Pass);
        }
    }
}
