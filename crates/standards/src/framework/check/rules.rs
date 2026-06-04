use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use specify_diagnostics::Diagnostic;

use crate::framework::builder::{framework_finding, loc};
use crate::framework::check::Check;
use crate::framework::context::Context;
use crate::framework::helpers::{
    relative_display, skill_frontmatter, under_symlink, walk_matching_files,
};
use crate::framework::schema::{SchemaError, SchemaId, ValidationError, validate_frontmatter};

pub const RULE_SCHEMA_VIOLATION: &str = "rules.schema-violation";
pub const RULE_NAMESPACE_OWNERSHIP_VIOLATION: &str = "rules.namespace-ownership-violation";
pub const RULE_DUPLICATE_RULE_ID: &str = "rules.duplicate-rule-id";

const SHARED_RULES_OWNER: &str = "universal";
const CORE_RULES_OWNER: &str = "core";

static RULE_HEADING_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^## Rule\s*$").expect("rule heading regex"));

static RULE_ID_NAMESPACE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([A-Z]+)-[0-9]{3}$").expect("rule id regex"));

/// Built-in (target + shared) namespace map. Source-axis owners are
/// discovered per-run because every adapter under
/// `adapters/sources/<name>/rules/` owns the shared `SRC-*` namespace, and we
/// refuse to hardcode source-adapter names. See [`namespace_owners`] for the
/// merged per-run view.
static BUILTIN_NAMESPACES: LazyLock<HashMap<&'static str, HashSet<&'static str>>> =
    LazyLock::new(|| {
        HashMap::from([
            (SHARED_RULES_OWNER, HashSet::from(["UNI"])),
            (CORE_RULES_OWNER, HashSet::from(["CORE"])),
            ("omnia", HashSet::from(["OMNIA", "RUST", "SEC"])),
            ("contracts", HashSet::from(["IFACE"])),
            ("vectis", HashSet::from(["VECTIS"])),
        ])
    });

/// Rule shape validation and rule namespace ownership.
pub struct RulesCheck;

impl Check for RulesCheck {
    fn run(&self, ctx: &Context) -> Vec<Diagnostic> {
        run_rules_namespace_check(ctx)
    }
}

/// Rule schema shape, `## Rule` heading, and duplicate-id detection.
/// Invoked by declarative `CORE-026` / `CORE-027` via
/// `kind: authoring-predicate`, not the framework `AuthoringProducer`.
pub fn run_rules_schema_check(ctx: &Context) -> Vec<Diagnostic> {
    let paths = match discover_rule_files(ctx) {
        Ok(paths) => paths,
        Err(error) => {
            return vec![framework_finding(
                RULE_SCHEMA_VIOLATION,
                format!("Rule discovery failed: {error}"),
                None,
            )];
        }
    };

    let mut findings = Vec::new();
    let mut ids_by_value: BTreeMap<String, Vec<String>> = BTreeMap::new();

    for path in paths {
        let rel = relative_display(ctx.framework_root(), &path);
        let content = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(source) => {
                findings.push(finding_at(
                    RULE_SCHEMA_VIOLATION,
                    format!("Rule: {rel} — cannot read: {source}"),
                    &path,
                ));
                continue;
            }
        };

        match validate_frontmatter(&path, SchemaId::Rule) {
            Ok(()) => {}
            Err(SchemaError::Infrastructure(error)) => {
                findings.push(finding_at(
                    RULE_SCHEMA_VIOLATION,
                    format!("Rule: {rel} — {error}"),
                    &path,
                ));
            }
            Err(SchemaError::Validation(errors)) => {
                for error in errors {
                    let detail = format_validation_error(&error);
                    let prefix = if error.message.contains("missing leading YAML frontmatter") {
                        "Rule"
                    } else {
                        "Rule frontmatter"
                    };
                    findings.push(finding_at(
                        RULE_SCHEMA_VIOLATION,
                        format!("{prefix}: {rel} — {detail}"),
                        &path,
                    ));
                }
            }
        }

        if let Some(body) = rule_body(&content)
            && !RULE_HEADING_RE.is_match(body)
        {
            findings.push(finding_at(
                RULE_SCHEMA_VIOLATION,
                format!("Rule body: {rel} — missing required '## Rule' heading"),
                &path,
            ));
        }

        let Some(frontmatter) = skill_frontmatter(&content) else {
            continue;
        };

        let Some(id) = frontmatter.get("id").and_then(|value| value.as_str()) else {
            continue;
        };

        let seen = ids_by_value.entry(id.to_string()).or_default();
        seen.push(rel);
    }

    for (id, paths) in ids_by_value {
        if paths.len() > 1 {
            findings.push(framework_finding(
                RULE_DUPLICATE_RULE_ID,
                format!("Rule duplicate id '{id}' across files: {}", paths.join(", ")),
                None,
            ));
        }
    }

    findings
}

/// FRAME reservation, dynamic source owners, and namespace placement.
/// Retained as the sole `AuthoringProducer` imperative bridge (CORE-009).
pub fn run_rules_namespace_check(ctx: &Context) -> Vec<Diagnostic> {
    let paths = match discover_rule_files(ctx) {
        Ok(paths) => paths,
        Err(error) => {
            return vec![framework_finding(
                RULE_NAMESPACE_OWNERSHIP_VIOLATION,
                format!("Rule discovery failed: {error}"),
                None,
            )];
        }
    };

    let namespaces = namespace_owners(ctx);
    let mut findings = Vec::new();

    for path in paths {
        let rel = relative_display(ctx.framework_root(), &path);
        let content = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(_) => continue,
        };

        let Some(frontmatter) = skill_frontmatter(&content) else {
            continue;
        };

        let Some(id) = frontmatter.get("id").and_then(|value| value.as_str()) else {
            continue;
        };

        let Some(owner) = namespace_owner_for_path(ctx, &path) else {
            continue;
        };

        if let Some(namespace) = namespace_for_rule_id(id)
            && namespace == "FRAME"
            && owner != SHARED_RULES_OWNER
        {
            findings.push(finding_at(
                RULE_NAMESPACE_OWNERSHIP_VIOLATION,
                format!(
                    "Rules namespace ownership: {rel} — FRAME-* ids are reserved for framework-repo declarative rules and may not be placed under adapter trees (got '{id}' under rules owner '{owner}')"
                ),
                &path,
            ));
            continue;
        }

        let Some(allowed_namespaces) = namespaces.get(owner.as_str()) else {
            findings.push(finding_at(
                RULE_NAMESPACE_OWNERSHIP_VIOLATION,
                format!(
                    "Rules namespace ownership: {rel} — rules owner '{owner}' has no configured namespace; update crates/standards/src/framework/check/rules.rs before adding first-party rules here"
                ),
                &path,
            ));
            continue;
        };

        if let Some(namespace) = namespace_for_rule_id(id)
            && !allowed_namespaces.contains(namespace)
        {
            findings.push(finding_at(
                RULE_NAMESPACE_OWNERSHIP_VIOLATION,
                format!(
                    "Rules namespace ownership: {rel} — rules owner '{owner}' may only use {} ids, got '{id}'",
                    namespace_list(allowed_namespaces)
                ),
                &path,
            ));
        }
    }

    findings
}

/// Full fused rules pass (schema + namespace). Used by integration tests
/// and `kind: authoring-predicate` for `rules.*` ids.
pub fn run_rules_check(ctx: &Context) -> Vec<Diagnostic> {
    let mut findings = run_rules_schema_check(ctx);
    findings.extend(run_rules_namespace_check(ctx));
    findings
}

/// Build the rules-owner → allowed-namespaces map for this run.
///
/// Target adapters and the shared `universal` owner come from
/// [`BUILTIN_NAMESPACES`]. Source adapters are discovered dynamically: every
/// directory under `adapters/sources/<name>/rules/` registers
/// `<name>` → `{"SRC"}`. The rules contract §Namespaces forbids hardcoding
/// source-adapter names here — `SRC-*` is the single shared source-axis
/// namespace in v1.
fn namespace_owners(ctx: &Context) -> HashMap<String, HashSet<&'static str>> {
    let mut owners: HashMap<String, HashSet<&'static str>> = BUILTIN_NAMESPACES
        .iter()
        .map(|(owner, namespaces)| ((*owner).to_string(), namespaces.clone()))
        .collect();

    for owner in source_rules_owners(ctx) {
        owners.entry(owner).or_insert_with(|| HashSet::from(["SRC"]));
    }

    owners
}

/// Discover source-adapter owners that contribute a rules overlay.
///
/// Returns the first-segment directory name (e.g. `documentation`) for every
/// `adapters/sources/<name>/` directory that contains a `rules/` subdirectory.
/// Matches the placement predicate in `is_rule_in_axis`: an owner is
/// only considered to contribute to the namespace map when a `rules/` subtree
/// exists for it, mirroring how `discover_rule_files` only yields rules
/// from such trees.
fn source_rules_owners(ctx: &Context) -> Vec<String> {
    let sources_dir = ctx.sources_dir();
    let Ok(entries) = fs::read_dir(&sources_dir) else {
        return Vec::new();
    };

    let mut owners = Vec::new();
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }
        if !entry.path().join("rules").is_dir() {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        owners.push(name);
    }
    owners.sort();
    owners
}

fn discover_rule_files(
    ctx: &Context,
) -> Result<Vec<PathBuf>, crate::framework::error::ToolingError> {
    let framework_root = ctx.framework_root();
    let mut paths = Vec::new();

    for axis_dir in [ctx.sources_dir(), ctx.targets_dir()] {
        let files = walk_matching_files(framework_root, &axis_dir, ".md")?;
        for path in files {
            if is_rules_readme(&path) {
                continue;
            }
            if is_rule_in_axis(&path, &axis_dir) {
                paths.push(path);
            }
        }
    }

    for owner in [SHARED_RULES_OWNER, CORE_RULES_OWNER] {
        let pack_dir = ctx.adapters_shared_dir().join("rules").join(owner);
        if !pack_dir.is_dir() {
            continue;
        }
        let files = walk_matching_files(framework_root, &pack_dir, ".md")?;
        for path in files {
            if is_rules_readme(&path) {
                continue;
            }
            if under_symlink(framework_root, &path)? {
                continue;
            }
            paths.push(path);
        }
    }

    paths.sort();
    paths.dedup();
    Ok(paths)
}

fn is_rules_readme(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("readme.md"))
}

fn is_rule_in_axis(path: &Path, axis_root: &Path) -> bool {
    let Ok(rel) = path.strip_prefix(axis_root) else {
        return false;
    };
    let parts: Vec<_> = rel
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => part.to_str(),
            _ => None,
        })
        .collect();
    parts.len() >= 3 && parts.get(1) == Some(&"rules")
}

fn namespace_owner_for_path(ctx: &Context, path: &Path) -> Option<String> {
    for axis_dir in [ctx.sources_dir(), ctx.targets_dir()] {
        if let Ok(rel) = path.strip_prefix(&axis_dir) {
            let parts: Vec<_> = rel
                .components()
                .filter_map(|component| match component {
                    Component::Normal(part) => part.to_str(),
                    _ => None,
                })
                .collect();
            if parts.len() >= 3 && parts.get(1) == Some(&"rules") {
                return parts.first().map(|part| (*part).to_string());
            }
        }
    }

    for owner in [SHARED_RULES_OWNER, CORE_RULES_OWNER] {
        let pack_dir = ctx.adapters_shared_dir().join("rules").join(owner);
        if path.strip_prefix(&pack_dir).is_ok() {
            return Some(owner.to_string());
        }
    }

    None
}

fn namespace_for_rule_id(id: &str) -> Option<&str> {
    RULE_ID_NAMESPACE_RE
        .captures(id)
        .and_then(|captures| captures.get(1))
        .map(|capture| capture.as_str())
}

fn namespace_list(namespaces: &HashSet<&'static str>) -> String {
    let mut values: Vec<_> = namespaces.iter().copied().collect();
    values.sort_unstable();
    values.into_iter().map(|namespace| format!("{namespace}-*")).collect::<Vec<_>>().join(", ")
}

fn rule_body(content: &str) -> Option<&str> {
    let rest = content.strip_prefix("---\n")?;
    let end = rest.find("\n---")?;
    Some(&rest[end + "\n---".len()..])
}

fn format_validation_error(error: &ValidationError) -> String {
    let at =
        if error.instance_path.is_empty() { "/".to_string() } else { error.instance_path.clone() };

    if error.message.contains("missing required property")
        || error.message.contains("unknown property")
    {
        return error.message.clone();
    }

    format!("{at} {}", error.message).trim().to_string()
}

fn finding_at(rule_id: &'static str, message: String, path: &Path) -> Diagnostic {
    framework_finding(rule_id, message, Some(loc(path, 1, None)))
}

#[cfg(test)]
mod tests;
