//! Pure link-registry checks for the `links-registry` framework-authoring
//! tool, lifted from the host CLI's retiring `framework::check::schema_links`
//! (`SchemaLinksCheck`) and the directive half of `framework::check::links`
//! (`LinksCheck`) imperative predicates
//! (Road B framework tool).
//!
//! The tool covers the link-registry family: CORE-018
//! (`links.brief-schema-link-resolve` — a `schemas.specify.dev` URL in an
//! adapter brief must resolve to a known tool-owned schema, a tool→schema
//! registry join) and CORE-020 (`links.unresolved-directive` — a
//! `<!-- skill: plugin:skill -->` directive must resolve against the
//! on-disk skill registry discovered under `plugins/`).
//!
//! Policy is `specify`-owned, never baked here: CORE-018's tool→schema
//! registry arrives as a parameter the entrypoint reads from the rule's
//! `config:` (forwarded by the `kind: tool` evaluator). CORE-020 joins
//! against a registry discovered from the tree, so it carries no policy.
//! The only literals in this crate are mechanism — the `adapters/`
//! sub-tree, the `schemas.specify.dev` URL grammar, the directive
//! grammar, and the `plugins/<plugin>/skills/<skill>/` layout.
//!
//! Carve-out posture: this crate owns its logic and depends only on
//! `serde` / `serde_json` / `regex`, never the host diagnostics crate
//! (`main.rs` renders the wire envelope).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;

/// Codex ids each check stamps onto its findings (closed `CORE-NNN`).
pub const RULE_BRIEF_SCHEMA_LINK_RESOLVE: &str = "CORE-018";
pub const RULE_UNRESOLVED_DIRECTIVE: &str = "CORE-020";

/// One link-registry violation: its codex `rule_id`, an optional
/// project-relative path, and a human-readable message. The caller
/// stamps the wire severity (always `important` for this family).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkFinding {
    /// Codex `CORE-NNN` id this finding belongs to.
    pub rule_id: &'static str,
    /// Project-relative, forward-slash path of the offending file.
    pub path: Option<String>,
    /// Operator-facing message describing the violation.
    pub message: String,
}

/// One tool→schema-name registry row the CORE-018 rule supplies in
/// `config:`: a tool name and the schema names it owns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownSchema {
    /// Tool that owns the listed schemas (e.g. `vectis`).
    pub tool: String,
    /// Schema base names the tool owns (e.g. `tokens`, `assets`).
    pub schemas: Vec<String>,
}

/// CORE-018: every `https://schemas.specify.dev/<tool>/<name>.schema.json`
/// URL in an adapter brief or reference must resolve to a known
/// tool-owned schema named in the supplied `registry`. URLs in fenced or
/// inline code are ignored.
#[must_use]
pub fn check_schema_links(project_dir: &Path, registry: &[KnownSchema]) -> Vec<LinkFinding> {
    let adapters_dir = project_dir.join("adapters");
    if !adapters_dir.is_dir() {
        return Vec::new();
    }

    let url_re = schema_url_pattern();
    let inline_re = inline_code_pattern();
    let mut findings = Vec::new();

    for path in walk_markdown(&adapters_dir) {
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        let rel = relative_display(project_dir, &path);

        let mut in_fence = false;
        for (line_idx, line) in content.lines().enumerate() {
            if line.trim_start().starts_with("```") {
                in_fence = !in_fence;
                continue;
            }
            if in_fence {
                continue;
            }
            let cleaned = inline_re.replace_all(line, "");
            for cap in url_re.captures_iter(&cleaned) {
                let tool = cap.get(1).map_or("", |m| m.as_str());
                let name_with_ext = cap.get(2).map_or("", |m| m.as_str());
                let name = name_with_ext.strip_suffix(".schema.json").unwrap_or(name_with_ext);
                if !is_known_schema(registry, tool, name) {
                    let url = cap.get(0).map_or("", |m| m.as_str());
                    findings.push(LinkFinding {
                        rule_id: RULE_BRIEF_SCHEMA_LINK_RESOLVE,
                        path: Some(rel.clone()),
                        message: format!(
                            "{rel}:{} — schema URL '{url}' does not resolve to a known \
                             tool-owned schema",
                            line_idx + 1,
                        ),
                    });
                }
            }
        }
    }

    findings
}

fn is_known_schema(registry: &[KnownSchema], tool: &str, name: &str) -> bool {
    registry.iter().any(|row| row.tool == tool && row.schemas.iter().any(|s| s == name))
}

/// CORE-020: every `<!-- skill: plugin:skill -->` directive across the
/// tree must resolve against the skill registry discovered under
/// `plugins/`. Directives in fenced or inline code are ignored.
#[must_use]
pub fn check_directives(project_dir: &Path) -> Vec<LinkFinding> {
    let directive_re = directive_pattern();
    let fence_re = fenced_code_pattern();
    let inline_re = inline_code_pattern();
    let registry = build_skill_registry(project_dir);

    let mut findings = Vec::new();
    for path in walk_markdown(project_dir) {
        let path_str = path.to_string_lossy().replace('\\', "/");
        if skip_path(&path_str) {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        let rel = relative_display(project_dir, &path);

        let no_fence = fence_re.replace_all(&content, "");
        let stripped = inline_re.replace_all(&no_fence, "").into_owned();

        for cap in directive_re.captures_iter(&stripped) {
            let plugin = cap.get(1).map_or("", |m| m.as_str());
            let skill = cap.get(2).map_or("", |m| m.as_str());
            match registry.get(plugin) {
                None => findings.push(LinkFinding {
                    rule_id: RULE_UNRESOLVED_DIRECTIVE,
                    path: Some(rel.clone()),
                    message: format!("Invalid skill directive: {rel} — plugin '{plugin}' not found"),
                }),
                Some(skills) if !skills.contains(skill) => findings.push(LinkFinding {
                    rule_id: RULE_UNRESOLVED_DIRECTIVE,
                    path: Some(rel.clone()),
                    message: format!(
                        "Invalid skill directive: {rel} — skill '{plugin}:{skill}' not found"
                    ),
                }),
                _ => {}
            }
        }
    }

    findings
}

/// Discover the `plugin -> {skill}` registry from
/// `plugins/<plugin>/skills/<skill>/SKILL.md` paths.
fn build_skill_registry(project_dir: &Path) -> BTreeMap<String, BTreeSet<String>> {
    let plugins_dir = project_dir.join("plugins");
    let mut registry: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for path in walk_files(&plugins_dir) {
        if path.file_name().and_then(|n| n.to_str()) != Some("SKILL.md") {
            continue;
        }
        let Ok(rel) = path.strip_prefix(&plugins_dir) else {
            continue;
        };
        let parts: Vec<&str> = rel.iter().filter_map(|c| c.to_str()).collect();
        if parts.len() >= 4 && parts[1] == "skills" {
            registry.entry(parts[0].to_string()).or_default().insert(parts[2].to_string());
        }
    }
    registry
}

/// Mirror the directive predicate's skip set: vendored / VCS / scratch
/// trees and the host's own test fixtures are never scanned.
fn skip_path(path: &str) -> bool {
    path.contains("node_modules")
        || path.contains("/.git/")
        || path.contains("temp")
        || path.contains("tooling/tests/fixtures")
        || path.contains("crates/standards/tests/fixtures")
}

/// Collect every `.md` file under `dir`, skipping symlinked paths.
fn walk_markdown(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = walk_files(dir)
        .into_iter()
        .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("md"))
        .collect();
    out.sort();
    out
}

/// Recursive file collector that never follows or records symlinks,
/// matching the host's `follow_links(false)` + symlink-skip walk.
fn walk_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    collect_files(dir, &mut out);
    out
}

fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) {
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
            collect_files(&path, out);
        } else if file_type.is_file() {
            out.push(path);
        }
    }
}

fn relative_display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root).unwrap_or(path).to_string_lossy().replace('\\', "/")
}

fn schema_url_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"https://schemas\.specify\.dev/([a-z][a-z0-9-]*)/([a-z][a-z0-9-]*\.schema\.json)",
        )
        .expect("valid schema URL pattern")
    })
}

fn directive_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"<!-- skill: ([a-z][a-z0-9-]*):([a-z][a-z0-9-]*) -->")
            .expect("valid directive pattern")
    })
}

fn fenced_code_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"```[\s\S]*?```").expect("valid fence pattern"))
}

fn inline_code_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"`[^`]+`").expect("valid inline code pattern"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vectis_registry() -> Vec<KnownSchema> {
        vec![KnownSchema {
            tool: "vectis".to_string(),
            schemas: vec!["tokens".to_string(), "assets".to_string(), "composition".to_string()],
        }]
    }

    fn write(root: &Path, rel: &str, body: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, body).expect("write");
    }

    #[test]
    fn schema_links_flag_unknown_tool_schema() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(
            dir.path(),
            "adapters/targets/vectis/briefs/build.md",
            "See https://schemas.specify.dev/vectis/unknown.schema.json for details.\n",
        );
        let findings = check_schema_links(dir.path(), &vectis_registry());
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule_id, RULE_BRIEF_SCHEMA_LINK_RESOLVE);
    }

    #[test]
    fn schema_links_accept_known_schema() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(
            dir.path(),
            "adapters/targets/vectis/briefs/build.md",
            "See https://schemas.specify.dev/vectis/tokens.schema.json for details.\n",
        );
        assert!(check_schema_links(dir.path(), &vectis_registry()).is_empty());
    }

    #[test]
    fn directives_flag_unknown_plugin() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(dir.path(), "docs/guide.md", "<!-- skill: ghost:refine -->\n");
        let findings = check_directives(dir.path());
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule_id, RULE_UNRESOLVED_DIRECTIVE);
        assert!(findings[0].message.contains("plugin 'ghost' not found"));
    }

    #[test]
    fn directives_accept_registered_skill() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(dir.path(), "plugins/spec/skills/refine/SKILL.md", "---\nname: refine\n---\n");
        write(dir.path(), "docs/guide.md", "<!-- skill: spec:refine -->\n");
        assert!(check_directives(dir.path()).is_empty());
    }

    #[test]
    fn directives_flag_unknown_skill_in_known_plugin() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(dir.path(), "plugins/spec/skills/refine/SKILL.md", "---\nname: refine\n---\n");
        write(dir.path(), "docs/guide.md", "<!-- skill: spec:ghost -->\n");
        let findings = check_directives(dir.path());
        assert_eq!(findings.len(), 1);
        assert!(findings[0].message.contains("skill 'spec:ghost' not found"));
    }
}
