//! Pure rule-tree checks for the `rules` framework-authoring tool
//! (Road B framework tool).
//!
//! The tool covers the whole-tree `rules.*` family:
//!
//! - CORE-009 (`rules.namespace-ownership-violation`) — each rule
//!   markdown file's id-namespace prefix must be authored only under the
//!   rules directory that owns that namespace. Covers all four
//!   branches: the reserved-namespace reservation (`FRAME-*`), dynamic
//!   source-owner discovery, the unknown-owner diagnostic, and the
//!   placement check.
//! - CORE-026 (`rules.duplicate-rule-id`) — a rule id declared in more
//!   than one rules markdown file is a whole-tree duplicate.
//! - CORE-053 (`rules.body-heading-missing`) — every rule markdown file's
//!   body must carry the verbatim `## Rule` heading the frontmatter schema
//!   (CORE-027) does not cover.
//!
//! Policy is `specify`-owned, never baked here: the owner→allowed-prefix
//! map, the source-axis prefixes, and the reserved-namespace owners all
//! arrive as the [`OwnerPolicy`] the entrypoint reads from CORE-009's
//! `config:` (forwarded by the `kind: tool` evaluator). No owner name,
//! id-namespace prefix, or reserved namespace is hard-coded in this
//! crate — only the filesystem mechanism (the `adapters/{sources,targets}`
//! axis layout, the shared `universal` / `core` pack directories, and the
//! `<adapter>/rules/` placement shape) is.
//!
//! Carve-out posture: this crate owns its logic and depends only on
//! `serde-saphyr` / `serde_json`, never the host diagnostics crate
//! (`main.rs` renders the wire envelope).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use serde_json::Value as JsonValue;

/// Codex ids each check stamps onto its findings (closed `CORE-NNN`).
pub const RULE_NAMESPACE_OWNERSHIP_VIOLATION: &str = "CORE-009";
pub const RULE_DUPLICATE_RULE_ID: &str = "CORE-026";
pub const RULE_BODY_HEADING_MISSING: &str = "CORE-053";

/// Shared `universal` / `core` rules-pack directory names under
/// `adapters/shared/rules/`. Mechanism (filesystem layout), not policy:
/// the owner→prefix mapping for these names lives in [`OwnerPolicy`].
const SHARED_PACKS: [&str; 2] = ["universal", "core"];

/// One rule-tree violation: its codex `rule_id`, an optional
/// project-relative path, and a human-readable message. The caller
/// stamps the wire severity (always `important` for this family).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RulesFinding {
    /// Codex `CORE-NNN` id this finding belongs to.
    pub rule_id: &'static str,
    /// Project-relative, forward-slash path of the offending file, or
    /// `None` for whole-tree findings (duplicate id).
    pub path: Option<String>,
    /// Operator-facing message describing the violation.
    pub message: String,
}

/// `specify`-owned namespace policy CORE-009 supplies in `config:`.
///
/// Every value here is framework policy relayed by the engine; the tool
/// embeds none of it. The directory→owner derivation that pairs a rule
/// file with an `owner_prefixes` key is mechanism and stays in code.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OwnerPolicy {
    /// `<rules-directory-owner> -> {allowed id-namespace prefixes}`.
    pub owner_prefixes: BTreeMap<String, BTreeSet<String>>,
    /// Id-namespace prefixes every dynamically-discovered source-adapter
    /// owner may use (e.g. `SRC`).
    pub source_axis_prefixes: BTreeSet<String>,
    /// `<reserved namespace> -> <sole owner>` (e.g. `FRAME -> universal`).
    pub reserved: BTreeMap<String, String>,
}

/// CORE-009: assert each rule file's id-namespace prefix is owned by its
/// containing rules directory. Walks the rule trees (target + source
/// axes, then the shared `universal` / `core` packs) and applies, in
/// order: reserved-namespace reservation, unknown-owner, placement.
#[must_use]
pub fn check_namespace_ownership(project_dir: &Path, policy: &OwnerPolicy) -> Vec<RulesFinding> {
    let mut findings = Vec::new();
    for path in discover_rule_files(project_dir) {
        let rel = relative_display(project_dir, &path);
        let Some(frontmatter) = read_frontmatter(&path) else {
            continue;
        };
        let Some(id) = frontmatter.get("id").and_then(JsonValue::as_str) else {
            continue;
        };
        let Some(owner) = owner_for_path(project_dir, &path) else {
            continue;
        };
        let Some(namespace) = namespace_prefix(id) else {
            continue;
        };

        if let Some(reserved_owner) = policy.reserved.get(namespace)
            && owner != *reserved_owner
        {
            findings.push(RulesFinding {
                rule_id: RULE_NAMESPACE_OWNERSHIP_VIOLATION,
                path: Some(rel.clone()),
                message: format!(
                    "Rules namespace ownership: {rel} — {namespace}-* ids are reserved for framework-repo declarative rules and may not be placed under adapter trees (got '{id}' under rules owner '{owner}')"
                ),
            });
            continue;
        }

        let Some(allowed) = allowed_prefixes(project_dir, &path, &owner, policy) else {
            findings.push(RulesFinding {
                rule_id: RULE_NAMESPACE_OWNERSHIP_VIOLATION,
                path: Some(rel.clone()),
                message: format!(
                    "Rules namespace ownership: {rel} — rules owner '{owner}' has no configured namespace; add it to the CORE-009 rule's owner-prefixes config before adding first-party rules here"
                ),
            });
            continue;
        };

        if !allowed.contains(namespace) {
            findings.push(RulesFinding {
                rule_id: RULE_NAMESPACE_OWNERSHIP_VIOLATION,
                path: Some(rel.clone()),
                message: format!(
                    "Rules namespace ownership: {rel} — rules owner '{owner}' may only use {} ids, got '{id}'",
                    namespace_list(allowed)
                ),
            });
        }
    }
    findings
}

/// CORE-026: a rule id declared in more than one rules markdown file is a
/// whole-tree duplicate.
#[must_use]
pub fn check_duplicate_rule_id(project_dir: &Path) -> Vec<RulesFinding> {
    let mut ids_by_value: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for path in discover_rule_files(project_dir) {
        let rel = relative_display(project_dir, &path);
        let Some(frontmatter) = read_frontmatter(&path) else {
            continue;
        };
        let Some(id) = frontmatter.get("id").and_then(JsonValue::as_str) else {
            continue;
        };
        ids_by_value.entry(id.to_string()).or_default().push(rel);
    }

    let mut findings = Vec::new();
    for (id, mut paths) in ids_by_value {
        if paths.len() > 1 {
            paths.sort();
            findings.push(RulesFinding {
                rule_id: RULE_DUPLICATE_RULE_ID,
                path: None,
                message: format!("Rule duplicate id '{id}' across files: {}", paths.join(", ")),
            });
        }
    }
    findings
}

/// CORE-053: every rules markdown file's body must carry the verbatim
/// `## Rule` heading on its own line. The frontmatter schema (CORE-027)
/// does not cover body conventions, so the heading is enforced here over
/// the same rule-tree walk as the namespace and duplicate-id checks.
#[must_use]
pub fn check_rule_body_heading(project_dir: &Path) -> Vec<RulesFinding> {
    let mut findings = Vec::new();
    for path in discover_rule_files(project_dir) {
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        if !has_rule_heading(&content) {
            let rel = relative_display(project_dir, &path);
            findings.push(RulesFinding {
                rule_id: RULE_BODY_HEADING_MISSING,
                path: Some(rel.clone()),
                message: format!(
                    "Rule body heading: {rel} — rule markdown must carry a verbatim `## Rule` heading in its body"
                ),
            });
        }
    }
    findings
}

/// True when the post-frontmatter body carries a `## Rule` heading on its
/// own line. Matches the host parser's body convention.
fn has_rule_heading(content: &str) -> bool {
    body_after_frontmatter(content).lines().any(|line| line == "## Rule")
}

/// The content after the YAML frontmatter block, or the whole content
/// when no frontmatter is present.
fn body_after_frontmatter(content: &str) -> &str {
    let Some(rest) = content.strip_prefix("---\n").or_else(|| content.strip_prefix("---\r\n"))
    else {
        return content;
    };
    let Some(end) = rest.find("\n---") else {
        return content;
    };
    let after_fence = &rest[end + 1..];
    match after_fence.find('\n') {
        Some(newline) => &after_fence[newline + 1..],
        None => "",
    }
}

/// Resolve the allowed id-prefix set for a rule file: the source-axis
/// prefixes for a source-adapter rule (dynamic owner discovery), else
/// the static `owner_prefixes` entry for the directory owner.
fn allowed_prefixes<'a>(
    project_dir: &Path, path: &Path, owner: &str, policy: &'a OwnerPolicy,
) -> Option<&'a BTreeSet<String>> {
    if is_source_rule(project_dir, path) {
        return Some(&policy.source_axis_prefixes);
    }
    policy.owner_prefixes.get(owner)
}

/// Discover every rules markdown file under the target / source axes
/// (`<adapter>/rules/**`) and the shared `universal` / `core` packs,
/// skipping `README.md`.
fn discover_rule_files(project_dir: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for axis in ["sources", "targets"] {
        let axis_dir = project_dir.join("adapters").join(axis);
        let mut files = Vec::new();
        walk_files(&axis_dir, &mut files);
        for path in files {
            if !is_markdown(&path) || is_rules_readme(&path) {
                continue;
            }
            if is_rule_in_axis(&path, &axis_dir) {
                paths.push(path);
            }
        }
    }
    for pack in SHARED_PACKS {
        let pack_dir = project_dir.join("adapters").join("shared").join("rules").join(pack);
        let mut files = Vec::new();
        walk_files(&pack_dir, &mut files);
        for path in files {
            if !is_markdown(&path) || is_rules_readme(&path) {
                continue;
            }
            paths.push(path);
        }
    }
    paths.sort();
    paths.dedup();
    paths
}

/// True when `path` sits at `<adapter>/rules/**` under `axis_root`
/// (relative depth ≥ 3 with `rules` as the second component).
fn is_rule_in_axis(path: &Path, axis_root: &Path) -> bool {
    let Ok(rel) = path.strip_prefix(axis_root) else {
        return false;
    };
    let parts = normal_parts(rel);
    parts.len() >= 3 && parts.get(1).copied() == Some("rules")
}

/// Resolve the rules-directory owner for `path`: the adapter name for an
/// axis rule, or the pack name for a shared-pack rule.
fn owner_for_path(project_dir: &Path, path: &Path) -> Option<String> {
    for axis in ["sources", "targets"] {
        let axis_dir = project_dir.join("adapters").join(axis);
        if let Ok(rel) = path.strip_prefix(&axis_dir) {
            let parts = normal_parts(rel);
            if parts.len() >= 3 && parts.get(1).copied() == Some("rules") {
                return parts.first().map(|part| (*part).to_string());
            }
        }
    }
    for pack in SHARED_PACKS {
        let pack_dir = project_dir.join("adapters").join("shared").join("rules").join(pack);
        if path.strip_prefix(&pack_dir).is_ok() {
            return Some(pack.to_string());
        }
    }
    None
}

/// True when `path` is a source-adapter rule (`adapters/sources/<name>/rules/**`).
fn is_source_rule(project_dir: &Path, path: &Path) -> bool {
    let axis_dir = project_dir.join("adapters").join("sources");
    is_rule_in_axis(path, &axis_dir)
}

/// Extract the `PREFIX` from a `PREFIX-NNN` rule id (uppercase ASCII
/// letters, hyphen, three digits). Returns `None` for any other shape so
/// malformed ids are left to the schema rule.
fn namespace_prefix(id: &str) -> Option<&str> {
    let (prefix, suffix) = id.split_once('-')?;
    let well_formed = !prefix.is_empty()
        && prefix.bytes().all(|b| b.is_ascii_uppercase())
        && suffix.len() == 3
        && suffix.bytes().all(|b| b.is_ascii_digit());
    well_formed.then_some(prefix)
}

/// Render the allowed set as a sorted, comma-joined `PREFIX-*` list.
fn namespace_list(namespaces: &BTreeSet<String>) -> String {
    namespaces.iter().map(|namespace| format!("{namespace}-*")).collect::<Vec<_>>().join(", ")
}

fn is_markdown(path: &Path) -> bool {
    path.extension().and_then(|ext| ext.to_str()).is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
}

fn is_rules_readme(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("readme.md"))
}

fn normal_parts(rel: &Path) -> Vec<&str> {
    rel.components()
        .filter_map(|component| match component {
            Component::Normal(part) => part.to_str(),
            _ => None,
        })
        .collect()
}

fn relative_display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root).unwrap_or(path).to_string_lossy().replace('\\', "/")
}

/// Read and parse a rule file's YAML frontmatter into a JSON map.
fn read_frontmatter(path: &Path) -> Option<BTreeMap<String, JsonValue>> {
    let content = std::fs::read_to_string(path).ok()?;
    let block = frontmatter_block(&content)?;
    serde_saphyr::from_str(block).ok()
}

/// Extract the YAML block between a leading `---` line and its closing
/// `---`. Mirrors the host frontmatter splitter.
fn frontmatter_block(content: &str) -> Option<&str> {
    let rest = content.strip_prefix("---\n").or_else(|| content.strip_prefix("---\r\n"))?;
    let end = rest.find("\n---")?;
    Some(&rest[..end])
}

/// Recursive file collector that never follows or records symlinks,
/// matching the host's `follow_links(false)` + symlink-skip discovery.
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

    fn write_rule(root: &Path, rel: &str, id: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().expect("rule parent")).expect("mkdir");
        let body = format!(
            "---\nid: {id}\ntitle: Fixture\nseverity: optional\ntrigger: Namespace fixture.\n---\n\n## Rule\n\nBody.\n"
        );
        std::fs::write(path, body).expect("write rule");
    }

    fn write_rule_without_heading(root: &Path, rel: &str, id: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().expect("rule parent")).expect("mkdir");
        let body = format!(
            "---\nid: {id}\ntitle: Fixture\nseverity: optional\ntrigger: Namespace fixture.\n---\n\nBody without the heading.\n"
        );
        std::fs::write(path, body).expect("write rule");
    }

    fn policy() -> OwnerPolicy {
        OwnerPolicy {
            owner_prefixes: BTreeMap::from([
                ("universal".to_string(), BTreeSet::from(["UNI".to_string()])),
                ("core".to_string(), BTreeSet::from(["CORE".to_string()])),
                (
                    "omnia".to_string(),
                    BTreeSet::from(["OMNIA".to_string(), "RUST".to_string(), "SEC".to_string()]),
                ),
                ("vectis".to_string(), BTreeSet::from(["VECTIS".to_string()])),
            ]),
            source_axis_prefixes: BTreeSet::from(["SRC".to_string()]),
            reserved: BTreeMap::from([("FRAME".to_string(), "universal".to_string())]),
        }
    }

    #[test]
    fn clean_tree_is_silent() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_rule(dir.path(), "adapters/shared/rules/core/CORE-001.md", "CORE-001");
        write_rule(dir.path(), "adapters/shared/rules/universal/UNI-001.md", "UNI-001");
        write_rule(dir.path(), "adapters/targets/omnia/rules/OMNIA-001.md", "OMNIA-001");
        write_rule(dir.path(), "adapters/sources/documentation/rules/SRC-001.md", "SRC-001");
        assert!(check_namespace_ownership(dir.path(), &policy()).is_empty());
        assert!(check_duplicate_rule_id(dir.path()).is_empty());
        assert!(check_rule_body_heading(dir.path()).is_empty());
    }

    #[test]
    fn flags_missing_rule_heading() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_rule(dir.path(), "adapters/shared/rules/core/CORE-001.md", "CORE-001");
        write_rule_without_heading(
            dir.path(),
            "adapters/shared/rules/universal/UNI-001.md",
            "UNI-001",
        );
        let findings = check_rule_body_heading(dir.path());
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule_id, RULE_BODY_HEADING_MISSING);
        assert_eq!(findings[0].path.as_deref(), Some("adapters/shared/rules/universal/UNI-001.md"));
        assert!(findings[0].message.contains("`## Rule`"));
    }

    #[test]
    fn flags_misplaced_prefix() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_rule(dir.path(), "adapters/targets/omnia/rules/VECTIS-001.md", "VECTIS-001");
        let findings = check_namespace_ownership(dir.path(), &policy());
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule_id, RULE_NAMESPACE_OWNERSHIP_VIOLATION);
        assert!(findings[0].message.contains("rules owner 'omnia' may only use"));
        assert!(findings[0].message.contains("VECTIS-001"));
    }

    #[test]
    fn flags_reserved_frame_under_adapter() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_rule(dir.path(), "adapters/targets/omnia/rules/FRAME-001.md", "FRAME-001");
        let findings = check_namespace_ownership(dir.path(), &policy());
        assert_eq!(findings.len(), 1);
        assert!(findings[0].message.contains("FRAME-* ids are reserved"));
        assert!(findings[0].message.contains("FRAME-001"));
    }

    #[test]
    fn flags_unknown_owner() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_rule(dir.path(), "adapters/targets/newadapter/rules/OMNIA-001.md", "OMNIA-001");
        let findings = check_namespace_ownership(dir.path(), &policy());
        assert_eq!(findings.len(), 1);
        assert!(findings[0].message.contains("has no configured namespace"));
    }

    #[test]
    fn flags_duplicate_id() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_rule(dir.path(), "adapters/shared/rules/core/first.md", "CORE-100");
        write_rule(dir.path(), "adapters/shared/rules/core/second.md", "CORE-100");
        let findings = check_duplicate_rule_id(dir.path());
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule_id, RULE_DUPLICATE_RULE_ID);
        assert!(findings[0].message.contains("CORE-100"));
        assert!(findings[0].message.contains("first.md"));
        assert!(findings[0].message.contains("second.md"));
    }
}
