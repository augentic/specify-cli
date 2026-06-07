//! Pure numeric-cap drift checks for the `prose` framework-authoring
//! tool, lifted from the host CLI's retiring `framework::check::prose`
//! (`NumericCaps`) imperative predicate
//! (Road B framework tool).
//!
//! The tool covers CORE-024 (`prose.numeric-cap-exceeded` — the
//! documented skill description / body line caps must stay in sync across
//! the embedded skill schema and the `docs/standards/skill-authoring.md`
//! prose). A bespoke numeric scan with no backing indexer fact.
//!
//! Policy is `specify`-owned, never baked here: the cap *values* arrive
//! as parameters the entrypoint reads from the rule's `config:`
//! (forwarded by the `kind: tool` evaluator). The only literals here are
//! mechanism — the standards-doc path and the embedded byte-identical
//! copy of `skill.schema.json` the description cap is cross-checked
//! against.
//!
//! Carve-out posture: this crate owns its logic and embeds its own copy
//! of `skill.schema.json`, depending only on `serde` / `serde_json`,
//! never the host diagnostics crate (`main.rs` renders the wire
//! envelope).

use std::path::Path;

/// Codex id every finding stamps (closed `CORE-NNN`).
pub const RULE_NUMERIC_CAP_EXCEEDED: &str = "CORE-024";

/// Tool-owned copy of the canonical skill frontmatter schema
/// (`schemas/authoring/skill.schema.json`). Embedded so the description
/// cap can be cross-checked against the schema source without reaching
/// back into the host engine (Road B B-2).
const SKILL_SCHEMA_SOURCE: &str = include_str!("../embedded/skill.schema.json");

/// Standards document that must carry both numeric caps in prose.
const STANDARDS_REL: &str = "docs/standards/skill-authoring.md";

/// One numeric-cap drift violation: its codex `rule_id`, an optional
/// project-relative path, and a human-readable message. The caller
/// stamps the wire severity (always `important`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProseFinding {
    /// Codex `CORE-NNN` id this finding belongs to.
    pub rule_id: &'static str,
    /// Project-relative, forward-slash path of the offending file, or
    /// `None` for the embedded-schema cross-check.
    pub path: Option<String>,
    /// Operator-facing message describing the violation.
    pub message: String,
}

/// CORE-024: the `description_cap` and `body_cap` policy values must be
/// present in the embedded skill schema (description only) and in the
/// standards document (both), or the documented caps have drifted.
#[must_use]
pub fn check_numeric_caps(
    project_dir: &Path, description_cap: u64, body_cap: u64,
) -> Vec<ProseFinding> {
    let mut findings = Vec::new();
    let description = description_cap.to_string();
    let body = body_cap.to_string();

    if !SKILL_SCHEMA_SOURCE.contains(&description) {
        findings.push(ProseFinding {
            rule_id: RULE_NUMERIC_CAP_EXCEEDED,
            path: None,
            message: format!(
                "Skill description cap drift in embedded skill.schema.json; expected {description}"
            ),
        });
    }

    let standards_path = project_dir.join(STANDARDS_REL);
    match std::fs::read_to_string(&standards_path) {
        Ok(content) => {
            if !content.contains(&description) {
                findings.push(standards_finding(format!(
                    "Skill description cap drift in {STANDARDS_REL}; expected {description}"
                )));
            }
            if !content.contains(&body) {
                findings.push(standards_finding(format!(
                    "Skill body cap drift in {STANDARDS_REL}; expected {body}"
                )));
            }
        }
        Err(_) => findings
            .push(standards_finding(format!("Skill numeric cap source missing: {STANDARDS_REL}"))),
    }

    findings
}

fn standards_finding(message: String) -> ProseFinding {
    ProseFinding {
        rule_id: RULE_NUMERIC_CAP_EXCEEDED,
        path: Some(STANDARDS_REL.to_string()),
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_standards(root: &Path, body: &str) {
        let path = root.join(STANDARDS_REL);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, body).expect("write");
    }

    #[test]
    fn clean_tree_is_silent() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_standards(dir.path(), "Description cap: 512 characters. Body cap: 200 lines.\n");
        assert!(check_numeric_caps(dir.path(), 512, 200).is_empty());
    }

    #[test]
    fn flags_body_cap_drift() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_standards(dir.path(), "Description cap: 512 characters.\n");
        let findings = check_numeric_caps(dir.path(), 512, 200);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule_id, RULE_NUMERIC_CAP_EXCEEDED);
        assert!(findings[0].message.contains("body cap drift"));
    }

    #[test]
    fn flags_missing_standards_doc() {
        let dir = tempfile::tempdir().expect("tempdir");
        let findings = check_numeric_caps(dir.path(), 512, 200);
        assert!(findings.iter().any(|f| f.message.contains("numeric cap source missing")));
    }
}
