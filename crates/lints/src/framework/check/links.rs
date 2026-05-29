use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;
use walkdir::WalkDir;

use crate::framework::builder::{framework_finding, loc};
use crate::framework::check::Check;
use crate::framework::context::Context;
use crate::framework::helpers::{walk_markdown_files, walk_skill_files};
use crate::rules::Diagnostic;

const RULE_BROKEN_REFERENCE: &str = "links.broken-reference";
const RULE_UNRESOLVED_DIRECTIVE: &str = "links.unresolved-directive";

/// Skill reference checks and skill directive validation.
///
/// Broken markdown link detection (formerly `links.unresolved`) was
/// retired in RFC-34 C10 — `CORE-002` ≅ `links.unresolved` now owns
/// that surface via a `path-pattern` + `reference-resolves`
/// deterministic hint pair (`adapters/shared/rules/core/CORE-002-links-unresolved.md`
/// in the framework repo). The parity test
/// `crates/lints/tests/core_parity_links_unresolved.rs` proves
/// the declarative interpreter flags the same unresolved-reference
/// set as the deleted imperative row, with the rule-id mapping
/// `links.unresolved` ↔ `CORE-002`. Skill reference and skill
/// directive checks below stay imperative until later cards map
/// them onto reserved-kind hints.
pub struct LinksCheck;

impl Check for LinksCheck {
    fn run(&self, ctx: &Context) -> Vec<Diagnostic> {
        run(ctx)
    }
}

/// Run all link predicates against `ctx`.
pub fn run(ctx: &Context) -> Vec<Diagnostic> {
    run_on_root(ctx.framework_root())
}

/// Run all link predicates against a framework root (used by integration tests).
pub fn run_on_root(root: &Path) -> Vec<Diagnostic> {
    let mut findings = check_references(root);
    findings.extend(check_directives(root));
    findings
}

fn check_references(root: &Path) -> Vec<Diagnostic> {
    let ref_re = reference_link_pattern();
    let fence_re = fenced_code_pattern();
    let plugins_dir = root.join("plugins");

    let mut findings = Vec::new();
    let skill_files = walk_skill_files(root).unwrap_or_default();
    for path in skill_files {
        let rel = path_relative(root, &path);
        let skill_dir = path.parent().unwrap_or(&plugins_dir);
        let content = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(source) => {
                findings.push(finding(
                    RULE_BROKEN_REFERENCE,
                    format!("Skill reference missing: {rel} — cannot read SKILL.md: {source}"),
                    path.clone(),
                ));
                continue;
            }
        };

        let stripped = fence_re.replace_all(&content, "");
        for cap in ref_re.captures_iter(&stripped) {
            let ref_path = cap.get(1).map(|m| m.as_str()).unwrap_or("");
            let ref_path = ref_path.split('#').next().unwrap_or("");
            if ref_path.is_empty() {
                continue;
            }
            let resolved = skill_dir.join(ref_path);
            if !resolved.exists() {
                findings.push(finding(
                    RULE_BROKEN_REFERENCE,
                    format!(
                        "Skill reference missing: {rel} links to '{ref_path}' but it doesn't exist"
                    ),
                    path.clone(),
                ));
            }
        }
    }

    findings
}

fn check_directives(root: &Path) -> Vec<Diagnostic> {
    let directive_re = directive_pattern();
    let fence_re = fenced_code_pattern();
    let inline_re = inline_code_pattern();
    let skip = directive_skip_patterns();
    let registry = build_skill_registry(root);

    let mut findings = Vec::new();
    for path in walk_markdown_files(root, root).unwrap_or_default() {
        let path_str = path_for_match(&path);
        if path_matches_any(&path_str, skip) || skip_test_fixtures(&path_str) {
            continue;
        }

        let rel = path_relative(root, &path);
        let content = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(_) => continue,
        };

        let stripped = {
            let no_fence = fence_re.replace_all(&content, "");
            inline_re.replace_all(&no_fence, "").into_owned()
        };

        for cap in directive_re.captures_iter(&stripped) {
            let plugin = cap.get(1).map(|m| m.as_str()).unwrap_or("");
            let skill = cap.get(2).map(|m| m.as_str()).unwrap_or("");
            match registry.get(plugin) {
                None => findings.push(finding(
                    RULE_UNRESOLVED_DIRECTIVE,
                    format!("Invalid skill directive: {rel} — plugin '{plugin}' not found"),
                    path.clone(),
                )),
                Some(skills) if !skills.contains(skill) => findings.push(finding(
                    RULE_UNRESOLVED_DIRECTIVE,
                    format!("Invalid skill directive: {rel} — skill '{plugin}:{skill}' not found"),
                    path.clone(),
                )),
                _ => {}
            }
        }
    }

    findings
}

fn build_skill_registry(root: &Path) -> HashMap<String, HashSet<String>> {
    let plugins_dir = root.join("plugins");
    let mut registry: HashMap<String, HashSet<String>> = HashMap::new();

    for entry in WalkDir::new(&plugins_dir).follow_links(false).into_iter().flatten() {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.into_path();
        if path.file_name().and_then(|n| n.to_str()) != Some("SKILL.md") {
            continue;
        }
        let rel = path
            .strip_prefix(&plugins_dir)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        let parts: Vec<&str> = rel.split('/').collect();
        if parts.len() >= 4 && parts[1] == "skills" {
            let plugin = parts[0].to_string();
            let skill = parts[2].to_string();
            registry.entry(plugin).or_default().insert(skill);
        }
    }

    registry
}

fn skip_test_fixtures(path: &str) -> bool {
    path.contains("tooling/tests/fixtures") || path.contains("crates/lints/tests/fixtures")
}

fn directive_skip_patterns() -> &'static [Regex] {
    static PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        ["node_modules", r"\.git", "temp"]
            .iter()
            .map(|pat| Regex::new(pat).expect("valid skip pattern"))
            .collect()
    })
}

fn path_matches_any(path: &str, patterns: &[Regex]) -> bool {
    patterns.iter().any(|re| re.is_match(path))
}

fn path_for_match(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn path_relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .map(|rel| rel.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| path.display().to_string())
}

fn reference_link_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"\[[^\]]*\]\((references/[^)]+|examples/[^)]+)\)")
            .expect("valid reference link pattern")
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

fn finding(rule_id: &'static str, message: String, path: PathBuf) -> Diagnostic {
    framework_finding(rule_id, message, Some(loc(path, 1, None)))
}
