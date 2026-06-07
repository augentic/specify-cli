//! Pure SKILL.md frontmatter checks for the `skill` framework-authoring
//! tool, lifted from the host CLI's retiring `skill_frontmatter`
//! imperative predicates (Road B framework tool).
//!
//! The tool covers the frontmatter family: CORE-042 (presence-only —
//! flags a SKILL.md whose leading YAML frontmatter is absent or
//! unparseable, kept disjoint from the already-migrated CORE-044
//! schema-on-present check), CORE-035 (`argument-hint` token grammar),
//! and CORE-036 (`description` first-verb allow-list). Each check mirrors
//! its counterpart in `framework::check::skill_frontmatter`; the
//! discovery walk mirrors the host's `walk_skill_files`.
//!
//! Policy is `specify`-owned, never baked here: CORE-035's token grammar
//! arrives as a regex string and CORE-036's verb allow-list as a string
//! list, both read by the entrypoint from the rule's `config:`
//! (forwarded by the `kind: tool` evaluator). CORE-042 is presence-only
//! and carries no policy.
//!
//! Carve-out posture: this crate owns its logic and depends only on
//! `serde` / `serde-saphyr` / `serde_json` / `regex`, never the host
//! diagnostics crate (`main.rs` renders the wire envelope).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use regex::Regex;
use serde_json::Value as JsonValue;

/// Codex ids each check stamps onto its findings (closed `CORE-NNN`).
pub const RULE_MISSING_FRONTMATTER: &str = "CORE-042";
pub const RULE_ARGUMENT_HINT_GRAMMAR: &str = "CORE-035";
pub const RULE_DESCRIPTION_GRAMMAR: &str = "CORE-036";

/// One skill-frontmatter violation: its codex `rule_id`, the offending
/// file's project-relative path, and a human-readable message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillFinding {
    /// Codex `CORE-NNN` id this finding belongs to.
    pub rule_id: &'static str,
    /// Project-relative, forward-slash path of the offending file.
    pub path: String,
    /// Operator-facing message describing the violation.
    pub message: String,
}

/// CORE-042: a SKILL.md whose leading YAML frontmatter is absent or
/// unparseable. Disjoint from CORE-044, which validates *present*
/// frontmatter against `skill.schema.json` via a native `kind: schema`
/// hint (and structurally skips files with no frontmatter fact).
#[must_use]
pub fn check_missing_frontmatter(project_dir: &Path) -> Vec<SkillFinding> {
    let mut findings = Vec::new();
    for path in walk_skill_files(project_dir) {
        let rel = relative_display(project_dir, &path);
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        if parse_frontmatter(&content).is_none() {
            findings.push(SkillFinding {
                rule_id: RULE_MISSING_FRONTMATTER,
                path: rel.clone(),
                message: format!(
                    "Skill frontmatter: {rel} — / missing leading YAML frontmatter delimited by ---"
                ),
            });
        }
    }
    findings
}

/// CORE-035: each whitespace-separated `argument-hint` token must match
/// the `specify`-owned grammar `token_pattern`.
#[must_use]
pub fn check_argument_hint_grammar(project_dir: &Path, token_pattern: &str) -> Vec<SkillFinding> {
    let token_re = match Regex::new(token_pattern) {
        Ok(re) => re,
        Err(err) => {
            return vec![SkillFinding {
                rule_id: RULE_ARGUMENT_HINT_GRAMMAR,
                path: String::new(),
                message: format!("Invalid argument-hint grammar pattern '{token_pattern}': {err}"),
            }];
        }
    };
    let mut findings = Vec::new();
    for path in walk_skill_files(project_dir) {
        let rel = relative_display(project_dir, &path);
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Some(frontmatter) = parse_frontmatter(&content) else {
            continue;
        };
        let Some(hint_value) = frontmatter.get("argument-hint") else {
            continue;
        };
        let Some(hint) = hint_value.as_str() else {
            findings.push(SkillFinding {
                rule_id: RULE_ARGUMENT_HINT_GRAMMAR,
                path: rel.clone(),
                message: format!("Invalid argument-hint type in {rel}: must be a string"),
            });
            continue;
        };
        if let Some(token) = first_invalid_token(hint, &token_re) {
            findings.push(SkillFinding {
                rule_id: RULE_ARGUMENT_HINT_GRAMMAR,
                path: rel.clone(),
                message: format!(
                    "Invalid argument-hint in {rel}: token '{token}' (in '{hint}') does not match grammar — allowed tokens are <name>, [name], <a|b>, [a|b], <name>..., [name]..., --flag (kebab-case names)"
                ),
            });
        }
    }
    findings
}

/// CORE-036: each skill's `description` must start with a verb in the
/// `specify`-owned `allowed_verbs` allow-list.
#[must_use]
pub fn check_description_grammar(project_dir: &Path, allowed_verbs: &[String]) -> Vec<SkillFinding> {
    let mut findings = Vec::new();
    for path in walk_skill_files(project_dir) {
        let rel = relative_display(project_dir, &path);
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Some(frontmatter) = parse_frontmatter(&content) else {
            continue;
        };
        let Some(description) = frontmatter.get("description").and_then(JsonValue::as_str) else {
            continue;
        };
        let trimmed = description.trim_start();
        let first_word = trimmed.split_whitespace().next().unwrap_or("");
        let first_alpha: String =
            first_word.chars().take_while(char::is_ascii_alphabetic).collect();
        if first_alpha.is_empty() {
            findings.push(SkillFinding {
                rule_id: RULE_DESCRIPTION_GRAMMAR,
                path: rel.clone(),
                message: format!(
                    "Skill description must start with an imperative verb: {rel} — no leading word found"
                ),
            });
            continue;
        }
        let lower = first_alpha.to_ascii_lowercase();
        if allowed_verbs.iter().any(|verb| verb == &lower) {
            continue;
        }
        findings.push(SkillFinding {
            rule_id: RULE_DESCRIPTION_GRAMMAR,
            path: rel.clone(),
            message: format!(
                "Skill description must start with an imperative verb: {rel} — '{first_alpha}' not in allow-list"
            ),
        });
    }
    findings
}

/// First whitespace-separated token of `hint` that fails `token_re`, or
/// `None` when every token matches (an empty hint passes vacuously).
fn first_invalid_token(hint: &str, token_re: &Regex) -> Option<String> {
    let trimmed = hint.trim();
    if trimmed.is_empty() {
        return None;
    }
    trimmed.split_whitespace().find(|token| !token_re.is_match(token)).map(str::to_string)
}

/// Parse a SKILL.md's leading YAML frontmatter into a string-keyed map,
/// or `None` when the block is absent or unparseable. Mirrors the host's
/// `skill_frontmatter`.
fn parse_frontmatter(content: &str) -> Option<BTreeMap<String, JsonValue>> {
    let block = frontmatter_block(content)?;
    serde_saphyr::from_str(block).ok()
}

fn frontmatter_block(content: &str) -> Option<&str> {
    let rest = content.strip_prefix("---\n").or_else(|| content.strip_prefix("---\r\n"))?;
    let end = rest.find("\n---")?;
    Some(&rest[..end])
}

fn relative_display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root).unwrap_or(path).to_string_lossy().replace('\\', "/")
}

/// Walk every `SKILL.md` under `<project_dir>/plugins`, never following
/// or collecting symlinks (mirrors the host's `walk_skill_files`).
fn walk_skill_files(project_dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    walk_files(&project_dir.join("plugins"), &mut out);
    out.retain(|path| path.file_name().and_then(|n| n.to_str()) == Some("SKILL.md"));
    out.sort();
    out
}

fn walk_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        let path = entry.path();
        if file_type.is_dir() {
            walk_files(&path, out);
        } else if file_type.is_file() {
            out.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN_PATTERN: &str = r"^(?:<[a-z][a-z0-9]*(?:-[a-z0-9]+)*(?:\|[a-z][a-z0-9]*(?:-[a-z0-9]+)*)*>(?:\.\.\.)?|\[[a-z][a-z0-9]*(?:-[a-z0-9]+)*(?:\|[a-z][a-z0-9]*(?:-[a-z0-9]+)*)*\](?:\.\.\.)?|--[a-z][a-z0-9]*(?:-[a-z0-9]+)*)$";

    fn write_raw_skill(dir: &Path, slug: &str, content: &str) {
        let skill_dir = dir.join("plugins/demo/skills").join(slug);
        std::fs::create_dir_all(&skill_dir).expect("skill dir");
        std::fs::write(skill_dir.join("SKILL.md"), content).expect("write skill");
    }

    #[test]
    fn missing_frontmatter_flags_blockless_skill() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_raw_skill(dir.path(), "bare", "# No frontmatter here\n");
        let findings = check_missing_frontmatter(dir.path());
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule_id, RULE_MISSING_FRONTMATTER);
    }

    #[test]
    fn missing_frontmatter_silent_with_block() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_raw_skill(dir.path(), "ok", "---\nname: ok\ndescription: x\n---\n\nBody.\n");
        assert!(check_missing_frontmatter(dir.path()).is_empty());
    }

    #[test]
    fn argument_hint_flags_prose() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_raw_skill(
            dir.path(),
            "hint",
            "---\nname: hint\ndescription: x\nargument-hint: the slice name\n---\n\nBody.\n",
        );
        let findings = check_argument_hint_grammar(dir.path(), TOKEN_PATTERN);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].message.contains("token 'the'"));
    }

    #[test]
    fn argument_hint_accepts_grammar() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_raw_skill(
            dir.path(),
            "hint",
            "---\nname: hint\ndescription: x\nargument-hint: <slice-dir> [crate-name]\n---\n\nBody.\n",
        );
        assert!(check_argument_hint_grammar(dir.path(), TOKEN_PATTERN).is_empty());
    }

    #[test]
    fn description_flags_non_verb() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_raw_skill(
            dir.path(),
            "desc",
            "---\nname: desc\ndescription: The thing that does work.\n---\n\nBody.\n",
        );
        let verbs = vec!["build".to_string(), "run".to_string()];
        let findings = check_description_grammar(dir.path(), &verbs);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].message.contains("'The' not in allow-list"));
    }

    #[test]
    fn description_accepts_allowed_verb() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_raw_skill(
            dir.path(),
            "desc",
            "---\nname: desc\ndescription: Build the demo fixtures.\n---\n\nBody.\n",
        );
        let verbs = vec!["build".to_string(), "run".to_string()];
        assert!(check_description_grammar(dir.path(), &verbs).is_empty());
    }
}
