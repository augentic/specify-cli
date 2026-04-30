//! Structural primitives — the small, unit-testable checks that named
//! rules compose.
//!
//! Each helper here is `pub(crate)` and side-effect free; the only I/O is
//! filesystem reads inside [`proposal_deliverables_have_specs`] and
//! [`design_references_exist`], both of which only consult `specs_dir`.

use std::path::Path;

use regex::Regex;
use specify_spec::ParsedSpec;
use specify_task::{Task, TaskProgress};

/// Return `true` if any line (after trimming trailing whitespace) equals
/// `heading` exactly. Case-sensitive — a spec that writes `## why` instead
/// of `## Why` is treated as missing the section, which is deliberate: we
/// only claim to understand the canonical casing from the brief.
///
/// Present in the primitives surface per RFC-1 line 677 even though the
/// initial registry only uses [`has_content_after_heading`]; future rules
/// that care about mere presence (e.g. optional sections) will compose it.
#[allow(dead_code)]
pub fn has_section(content: &str, heading: &str) -> bool {
    content.lines().any(|line| line.trim_end() == heading)
}

/// Return `true` when `heading` appears AND at least one non-empty,
/// non-whitespace line follows it before the next `##`-or-higher heading.
/// Blank lines between the heading and prose are fine.
pub fn has_content_after_heading(content: &str, heading: &str) -> bool {
    let mut lines = content.lines();
    while let Some(line) = lines.next() {
        if line.trim_end() != heading {
            continue;
        }
        // We've seen the heading; look ahead for prose.
        for follow in lines.by_ref() {
            let trimmed = follow.trim();
            if trimmed.is_empty() {
                continue;
            }
            if is_next_section_boundary(follow, heading) {
                // Hit a sibling/ancestor heading before finding prose.
                return false;
            }
            return true;
        }
        return false;
    }
    false
}

/// A heading line `##` or deeper that isn't the *same* heading we're
/// parsing is treated as the end of the current section. We compare levels
/// by counting leading `#`s: once we see a heading whose level is ≤ the
/// level of `current`, we've left the section.
fn is_next_section_boundary(line: &str, current: &str) -> bool {
    let current_level = leading_hash_count(current);
    let candidate_level = leading_hash_count(line.trim_start());
    // Not a heading at all.
    if candidate_level == 0 {
        return false;
    }
    candidate_level <= current_level
}

fn leading_hash_count(line: &str) -> usize {
    let trimmed = line.trim_start();
    let count = trimmed.chars().take_while(|c| *c == '#').count();
    // Require that the '#'s are followed by a space (or end-of-line) for
    // this to count as a heading; otherwise `#hashtag` false-matches.
    if count == 0 {
        return 0;
    }
    let rest = &trimmed[count..];
    if rest.is_empty() || rest.starts_with(' ') || rest.starts_with('\t') { count } else { 0 }
}

pub fn all_requirements_have_scenarios(spec: &ParsedSpec) -> bool {
    spec.requirements.iter().all(|r| !r.scenarios.is_empty())
}

pub fn all_requirements_have_ids(spec: &ParsedSpec) -> bool {
    spec.requirements.iter().all(|r| !r.id.is_empty())
}

/// Compile `pattern` as a regex and return `true` iff every requirement's
/// `id` fully matches. Invalid patterns (programmer error) return `false`.
pub fn ids_match_pattern(spec: &ParsedSpec, pattern: &str) -> bool {
    let Ok(re) = Regex::new(pattern) else {
        return false;
    };
    spec.requirements.iter().all(|r| {
        let Some(m) = re.find(&r.id) else {
            return false;
        };
        m.start() == 0 && m.end() == r.id.len()
    })
}

/// `true` iff every line starting with `-` in `content` was recognised by
/// the task parser (i.e. it's a `- [ ] X.Y …` checkbox). Non-checkbox
/// bullets like `- bare item` cause a `false` return.
///
/// Also returns `false` if the parsed total disagrees with the recognised
/// count (defensive — shouldn't happen by construction).
pub fn all_tasks_use_checkbox(tasks: &TaskProgress, content: &str) -> bool {
    if tasks.total != tasks.tasks.len() {
        return false;
    }
    let bullet_re = Regex::new(r"^\s*-\s+\S").expect("bullet regex is valid");
    let checkbox_re =
        Regex::new(r"^\s*-\s+\[( |x|X)\]\s+\d+(?:\.\d+)*\s+").expect("checkbox regex is valid");
    for line in content.lines() {
        if bullet_re.is_match(line) && !checkbox_re.is_match(line) {
            return false;
        }
    }
    true
}

pub fn tasks_grouped_under_headings(tasks: &TaskProgress) -> bool {
    tasks.tasks.iter().all(|t| !t.group.is_empty())
}

pub fn first_human_only_task(tasks: &TaskProgress) -> Option<String> {
    const FORBIDDEN: &[&str] = &[
        "manual",
        "manually",
        "human",
        "real api",
        "real-world api",
        "real world api",
        "production api",
        "production credential",
        "physical device",
        "visual inspection",
        "visually inspect",
        "app store",
        "ask the user",
        "user confirmation",
    ];

    tasks.tasks.iter().find_map(|task| {
        let description = task.description.to_ascii_lowercase();
        FORBIDDEN
            .iter()
            .any(|phrase| description.contains(phrase))
            .then(|| format!("{} {}", task.number, task.description))
    })
}

pub fn tasks_are_agent_completable(tasks: &TaskProgress) -> bool {
    first_human_only_task(tasks).is_none()
}

pub fn tasks_have_verification_path(tasks: &TaskProgress) -> bool {
    tasks.tasks.iter().any(task_has_verification_signal)
}

fn task_has_verification_signal(task: &Task) -> bool {
    if let Some(directive) = &task.skill_directive {
        let skill = directive.skill.as_str();
        if skill.contains("test")
            || skill.contains("review")
            || skill.contains("validator")
            || skill.contains("verify")
        {
            return true;
        }
    }

    const SIGNALS: &[&str] = &[
        "test",
        "tests",
        "verify",
        "verification",
        "validate",
        "validator",
        "review",
        "reviewer",
        "fixture",
        "fixtures",
        "mock",
        "mocks",
        "contract",
        "build",
        "check",
        "clippy",
        "fmt",
        "lint",
        "cargo",
        "gradle",
        "xcode",
        "swift",
        "assemble",
    ];

    let description = task.description.to_ascii_lowercase();
    SIGNALS.iter().any(|signal| description.contains(signal))
}

/// Return `true` iff every crate/feature entry listed under the
/// proposal's `## Crates` (or `## Features`) section has a matching
/// `specs/<name>/spec.md` on disk. If no deliverable section is present,
/// or the section is empty, returns `true` — the sibling
/// `has-content-after-heading` rule is responsible for that case.
pub fn proposal_deliverables_have_specs(proposal: &str, specs_dir: &Path, term: &str) -> bool {
    let headings: Vec<&str> = match term {
        "crate" => vec!["## Crates"],
        "feature" => vec!["## Features"],
        _ => vec!["## Crates", "## Features"],
    };

    for heading in headings {
        let entries = extract_deliverables(proposal, heading);
        if entries.is_empty() {
            continue;
        }
        for name in entries {
            let spec_path = specs_dir.join(&name).join("spec.md");
            if !spec_path.exists() {
                return false;
            }
        }
    }
    true
}

/// Parse the proposal for entries under `heading`. Accepts `- name`,
/// `` - `name` ``, or sub-headings (`### New Crates` / `### Modified Crates`)
/// whose bullets are in turn parsed. Placeholder tokens (values that look
/// like HTML comments) are skipped.
fn extract_deliverables(proposal: &str, heading: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut in_section = false;
    let mut section_level = 0usize;
    for line in proposal.lines() {
        let trimmed_end = line.trim_end();
        if !in_section {
            if trimmed_end == heading {
                in_section = true;
                section_level = leading_hash_count(heading);
            }
            continue;
        }
        // Leaving the section on a sibling/ancestor heading.
        let level = leading_hash_count(line.trim_start());
        if level > 0 && level <= section_level {
            break;
        }
        let content = line.trim();
        let Some(rest) = content.strip_prefix("- ") else {
            continue;
        };
        let rest = rest.trim();
        if rest.is_empty() {
            continue;
        }
        // Skip comment-shaped placeholder lines.
        if rest.starts_with("<!--") {
            continue;
        }
        // Accept either `- name`, `- `name`` (backtick-wrapped), or
        // `- **name**`. Split on whitespace and pick the first token,
        // stripping decorations.
        let first_token = rest.split_whitespace().next().unwrap_or("");
        let cleaned =
            first_token.trim_matches(|c: char| c == '`' || c == '*' || c == ':' || c == ',').trim();
        if cleaned.is_empty() {
            continue;
        }
        out.push(cleaned.to_string());
    }
    out
}

/// Extract the `$ref` target path from a YAML line like
/// `$ref: "../schemas/user.yaml"`. Returns `None` when the line is not a
/// `$ref:` entry or the value is empty/non-file (fragment-only or URL).
pub fn extract_ref(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    let rest = trimmed.strip_prefix("$ref:")?;
    let value = rest.trim().trim_matches('"').trim_matches('\'');
    if value.is_empty() || value.starts_with('#') || value.contains("://") {
        return None;
    }
    let path_part = value.split('#').next().unwrap_or(value);
    if path_part.is_empty() {
        return None;
    }
    Some(path_part)
}

/// Match `REQ-XXX` IDs in the design doc; return `true` iff each is present
/// in at least one `specs/*/spec.md` under `specs_dir`. Returns `true` if
/// no references are found.
pub fn design_references_exist(design: &str, specs_dir: &Path) -> bool {
    let re = Regex::new(r"REQ-[0-9]{3}").expect("req id regex is valid");
    let mut refs: Vec<String> = re.find_iter(design).map(|m| m.as_str().to_string()).collect();
    refs.sort();
    refs.dedup();
    if refs.is_empty() {
        return true;
    }
    let Ok(dir_iter) = std::fs::read_dir(specs_dir) else {
        return false;
    };
    let mut spec_bodies: Vec<String> = Vec::new();
    for entry in dir_iter.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let spec_path = path.join("spec.md");
        if let Ok(contents) = std::fs::read_to_string(&spec_path) {
            spec_bodies.push(contents);
        }
    }
    refs.iter().all(|needle| spec_bodies.iter().any(|body| body.contains(needle)))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;

    fn tmp() -> TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    #[test]
    fn section_exact_line() {
        assert!(has_section("## Why\nbecause", "## Why"));
        assert!(!has_section("## why\nbecause", "## Why"));
        assert!(!has_section("prose without heading", "## Why"));
    }

    #[test]
    fn content_after_heading_prose() {
        let ok = "## Why\n\nbecause the problem exists\n\n## Next\n";
        assert!(has_content_after_heading(ok, "## Why"));
        let empty = "## Why\n\n## Next\nstuff\n";
        assert!(!has_content_after_heading(empty, "## Why"));
        let subheading_only = "## Why\n### Child\n\n## Next\n";
        // `### Child` is a child heading (deeper), prose-ish enough.
        assert!(has_content_after_heading(subheading_only, "## Why"));
        assert!(!has_content_after_heading("no such heading", "## Why"));
    }

    #[test]
    fn requirements_need_scenarios() {
        let ok = specify_spec::parse_baseline(
            "### Requirement: Thing\n\nID: REQ-001\n\n#### Scenario: Happy\n- WHEN foo\n- THEN bar\n",
        );
        assert!(all_requirements_have_scenarios(&ok));
        let bad =
            specify_spec::parse_baseline("### Requirement: Thing\n\nID: REQ-001\n\nno scenario\n");
        assert!(!all_requirements_have_scenarios(&bad));
    }

    #[test]
    fn requirements_need_ids() {
        let ok = specify_spec::parse_baseline(
            "### Requirement: Thing\n\nID: REQ-001\n\n#### Scenario: Happy\n",
        );
        assert!(all_requirements_have_ids(&ok));
        let bad = specify_spec::parse_baseline("### Requirement: Thing\n\n#### Scenario: Happy\n");
        assert!(!all_requirements_have_ids(&bad));
    }

    #[test]
    fn req_id_pattern() {
        let ok = specify_spec::parse_baseline(
            "### Requirement: Thing\n\nID: REQ-001\n\n#### Scenario: Happy\n",
        );
        assert!(ids_match_pattern(&ok, specify_spec::REQUIREMENT_ID_PATTERN));
        let bad = specify_spec::parse_baseline(
            "### Requirement: Thing\n\nID: REQ-1\n\n#### Scenario: Happy\n",
        );
        assert!(!ids_match_pattern(&bad, specify_spec::REQUIREMENT_ID_PATTERN));
    }

    #[test]
    fn checkbox_rejects_bare_bullets() {
        let ok = "## 1. Setup\n- [ ] 1.1 Do thing\n- [ ] 1.2 Do other\n";
        let progress = specify_task::parse_tasks(ok);
        assert!(all_tasks_use_checkbox(&progress, ok));
        let bad = "## 1. Setup\n- [ ] 1.1 Do thing\n- bare bullet\n";
        let progress = specify_task::parse_tasks(bad);
        assert!(!all_tasks_use_checkbox(&progress, bad));
    }

    #[test]
    fn tasks_require_group_headings() {
        let ok = "## 1. Setup\n- [ ] 1.1 Do thing\n";
        let progress = specify_task::parse_tasks(ok);
        assert!(tasks_grouped_under_headings(&progress));
        let bad = "- [ ] 1.1 Do thing\n";
        let progress = specify_task::parse_tasks(bad);
        assert!(!tasks_grouped_under_headings(&progress));
    }

    #[test]
    fn tasks_reject_human_only_mobile_api_checks() {
        let input = "\
## 1. Verify

- [ ] 1.1 Manually test the iOS and Android apps against the real API
";
        let progress = specify_task::parse_tasks(input);
        assert!(!tasks_are_agent_completable(&progress));
        assert_eq!(
            first_human_only_task(&progress),
            Some(
                "1.1 Manually test the iOS and Android apps against the real API".to_string()
            )
        );
    }

    #[test]
    fn tasks_accept_fixture_backed_agent_checks() {
        let input = "\
## 1. Verify

- [ ] 1.1 Add fixture-backed effect tests for API success and failure responses <!-- skill: vectis:test-writer -->
- [ ] 1.2 Verify iOS and Android shells build against the generated core
";
        let progress = specify_task::parse_tasks(input);
        assert!(tasks_are_agent_completable(&progress));
        assert!(tasks_have_verification_path(&progress));
    }

    #[test]
    fn tasks_need_an_agent_verification_path() {
        let input = "\
## 1. Implement

- [ ] 1.1 Create the login crate skeleton
- [ ] 1.2 Wire the crate into the workspace
";
        let progress = specify_task::parse_tasks(input);
        assert!(!tasks_have_verification_path(&progress));
    }

    #[test]
    fn deliverable_specs_on_disk() {
        let dir = tmp();
        let specs = dir.path().join("specs");
        fs::create_dir_all(specs.join("login")).unwrap();
        fs::write(specs.join("login").join("spec.md"), "# Login\n").unwrap();

        let ok_proposal = "## Crates\n\n- login\n";
        assert!(proposal_deliverables_have_specs(ok_proposal, &specs, "crate"));

        let missing = "## Crates\n\n- login\n- missing\n";
        assert!(!proposal_deliverables_have_specs(missing, &specs, "crate"));

        // Absent section → true (the has-content rule handles this case).
        let absent = "## Why\n\nbecause\n";
        assert!(proposal_deliverables_have_specs(absent, &specs, "crate"));
    }

    #[test]
    fn deliverables_backticked_names() {
        let dir = tmp();
        let specs = dir.path().join("specs");
        fs::create_dir_all(specs.join("user-auth")).unwrap();
        fs::write(specs.join("user-auth").join("spec.md"), "# Login\n").unwrap();
        let proposal = "## Crates\n\n### New Crates\n\n- `user-auth`\n";
        assert!(proposal_deliverables_have_specs(proposal, &specs, "crate"));
    }

    #[test]
    fn extract_ref_double_quoted() {
        assert_eq!(
            extract_ref(r#"              $ref: "../schemas/user.yaml""#),
            Some("../schemas/user.yaml")
        );
    }

    #[test]
    fn extract_ref_single_quoted() {
        assert_eq!(extract_ref("    $ref: '../schemas/user.yaml'"), Some("../schemas/user.yaml"));
    }

    #[test]
    fn extract_ref_unquoted() {
        assert_eq!(extract_ref("  $ref: ../schemas/user.yaml"), Some("../schemas/user.yaml"));
    }

    #[test]
    fn extract_ref_fragment() {
        assert_eq!(extract_ref("$ref: \"#/components/schemas/Error\""), None);
    }

    #[test]
    fn extract_ref_http_url() {
        assert_eq!(extract_ref("$ref: \"https://example.com/schemas/user.yaml\""), None);
    }

    #[test]
    fn extract_ref_non_ref_line() {
        assert_eq!(extract_ref("  type: string"), None);
        assert_eq!(extract_ref("  description: a $ref example"), None);
    }

    #[test]
    fn design_refs_backed_by_specs() {
        let dir = tmp();
        let specs = dir.path().join("specs");
        fs::create_dir_all(specs.join("a")).unwrap();
        fs::write(specs.join("a").join("spec.md"), "### Requirement: X\nID: REQ-001\n").unwrap();

        let ok = "See REQ-001 in the spec.";
        assert!(design_references_exist(ok, &specs));

        let missing = "See REQ-999 in the spec.";
        assert!(!design_references_exist(missing, &specs));

        let none = "No references in this doc.";
        assert!(design_references_exist(none, &specs));
    }
}
