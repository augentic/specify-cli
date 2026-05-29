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
        run_rules_check(ctx)
    }
}

pub fn run_rules_check(ctx: &Context) -> Vec<Diagnostic> {
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

    let namespaces = namespace_owners(ctx);
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

        match validate_frontmatter(ctx, &path, SchemaId::Rule) {
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
        seen.push(rel.clone());

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
                    "Rules namespace ownership: {rel} — rules owner '{owner}' has no configured namespace; update crates/lints/src/framework/check/rules.rs before adding first-party rules here"
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
mod tests {
    use std::fs;
    use std::path::Path;

    use tempfile::TempDir;

    use super::*;
    use crate::framework::builder::{core_id_for, snippet};

    #[test]
    fn namespace_for_rule_id_extracts_prefix() {
        assert_eq!(namespace_for_rule_id("UNI-014"), Some("UNI"));
        assert_eq!(namespace_for_rule_id("OMNIA-001"), Some("OMNIA"));
        assert_eq!(namespace_for_rule_id("bad"), None);
    }

    #[test]
    fn namespace_list_formats_wildcards() {
        let namespaces = HashSet::from(["OMNIA", "RUST", "SEC"]);
        assert_eq!(namespace_list(&namespaces), "OMNIA-*, RUST-*, SEC-*");
    }

    fn scaffold_framework(root: &Path) {
        fs::create_dir_all(root.join("adapters/sources")).expect("sources dir");
        fs::create_dir_all(root.join("adapters/targets")).expect("targets dir");
        fs::create_dir_all(root.join("adapters/shared")).expect("shared dir");
        fs::create_dir_all(root.join("plugins")).expect("plugins dir");
    }

    fn write_rule(root: &Path, rel: &str, id: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().expect("rule parent dir")).expect("create parent");
        let body = format!(
            "---\nid: {id}\ntitle: Test Rule\nseverity: important\ntrigger: When testing codex validation in specdev lint.\n---\n\n## Rule\n\nBody.\n"
        );
        fs::write(path, body).expect("write rule");
    }

    fn ctx_for(root: &Path) -> Context {
        Context::from_framework_root(root).expect("framework root")
    }

    #[test]
    fn owners_merge_builtins_and_discovered() {
        let temp = TempDir::new().expect("tempdir");
        scaffold_framework(temp.path());
        fs::create_dir_all(temp.path().join("adapters/sources/documentation/rules"))
            .expect("documentation rules");
        fs::create_dir_all(temp.path().join("adapters/sources/captures/rules"))
            .expect("captures rules");
        fs::create_dir_all(temp.path().join("adapters/sources/intent")).expect("intent no rules");

        let ctx = ctx_for(temp.path());
        let owners = namespace_owners(&ctx);

        assert_eq!(owners.get("documentation"), Some(&HashSet::from(["SRC"])));
        assert_eq!(owners.get("captures"), Some(&HashSet::from(["SRC"])));
        assert!(
            !owners.contains_key("intent"),
            "intent has no rules/ subtree so it must not be registered",
        );
        assert_eq!(owners.get(SHARED_RULES_OWNER), Some(&HashSet::from(["UNI"])));
        assert_eq!(owners.get("omnia"), Some(&HashSet::from(["OMNIA", "RUST", "SEC"])));
        assert_eq!(owners.get("vectis"), Some(&HashSet::from(["VECTIS"])));
        assert_eq!(owners.get("contracts"), Some(&HashSet::from(["IFACE"])));
    }

    #[test]
    fn src_rule_on_source_passes() {
        let temp = TempDir::new().expect("tempdir");
        scaffold_framework(temp.path());
        write_rule(
            temp.path(),
            "adapters/sources/documentation/rules/source-overlay.md",
            "SRC-001",
        );

        let findings = run_rules_check(&ctx_for(temp.path()));
        let ownership: Vec<_> = findings
            .iter()
            .filter(|finding| {
                finding.rule_id.as_deref() == core_id_for(RULE_NAMESPACE_OWNERSHIP_VIOLATION)
            })
            .collect();
        assert!(
            ownership.is_empty(),
            "SRC-* under source-adapter rules should pass, got: {ownership:?}",
        );
    }

    #[test]
    fn non_src_rule_under_source_adapter_rejected() {
        let temp = TempDir::new().expect("tempdir");
        scaffold_framework(temp.path());
        write_rule(
            temp.path(),
            "adapters/sources/documentation/rules/wrong-namespace.md",
            "OMNIA-001",
        );

        let findings = run_rules_check(&ctx_for(temp.path()));
        assert!(
            findings.iter().any(|finding| {
                finding.rule_id.as_deref() == core_id_for(RULE_NAMESPACE_OWNERSHIP_VIOLATION)
                    && snippet(finding).contains("rules owner 'documentation' may only use")
                    && snippet(finding).contains("SRC-*")
                    && snippet(finding).contains("OMNIA-001")
            }),
            "expected SRC-only enforcement under source adapter, got: {findings:?}",
        );
    }

    #[test]
    fn frame_rule_on_target_rejected() {
        let temp = TempDir::new().expect("tempdir");
        scaffold_framework(temp.path());
        write_rule(temp.path(), "adapters/targets/omnia/rules/frame-misplaced.md", "FRAME-001");

        let findings = run_rules_check(&ctx_for(temp.path()));
        assert!(
            findings.iter().any(|finding| {
                finding.rule_id.as_deref() == core_id_for(RULE_NAMESPACE_OWNERSHIP_VIOLATION)
                    && snippet(finding).contains("FRAME-*")
                    && snippet(finding).contains("framework-repo declarative rules")
                    && snippet(finding).contains("FRAME-001")
                    && snippet(finding).contains("omnia")
            }),
            "expected FRAME placement violation with framework rule-namespace reservation message, got: {findings:?}",
        );
    }

    #[test]
    fn frame_rule_on_source_rejected() {
        let temp = TempDir::new().expect("tempdir");
        scaffold_framework(temp.path());
        write_rule(
            temp.path(),
            "adapters/sources/documentation/rules/frame-misplaced.md",
            "FRAME-007",
        );

        let findings = run_rules_check(&ctx_for(temp.path()));
        assert!(
            findings.iter().any(|finding| {
                finding.rule_id.as_deref() == core_id_for(RULE_NAMESPACE_OWNERSHIP_VIOLATION)
                    && snippet(finding).contains("FRAME-*")
                    && snippet(finding).contains("framework-repo declarative rules")
                    && snippet(finding).contains("FRAME-007")
                    && snippet(finding).contains("documentation")
            }),
            "expected FRAME placement violation under source adapter, got: {findings:?}",
        );
    }

    #[test]
    fn core_rule_under_core_pack_passes() {
        let temp = TempDir::new().expect("tempdir");
        scaffold_framework(temp.path());
        write_rule(temp.path(), "adapters/shared/rules/core/CORE-fixture.md", "CORE-001");

        let findings = run_rules_check(&ctx_for(temp.path()));
        let ownership: Vec<_> = findings
            .iter()
            .filter(|finding| {
                finding.rule_id.as_deref() == core_id_for(RULE_NAMESPACE_OWNERSHIP_VIOLATION)
            })
            .collect();
        assert!(
            ownership.is_empty(),
            "CORE-* under adapters/shared/rules/core/ should pass, got: {ownership:?}",
        );
    }

    #[test]
    fn core_rule_under_target_adapter_rejected() {
        let temp = TempDir::new().expect("tempdir");
        scaffold_framework(temp.path());
        write_rule(temp.path(), "adapters/targets/omnia/rules/core-misplaced.md", "CORE-001");

        let findings = run_rules_check(&ctx_for(temp.path()));
        assert!(
            findings.iter().any(|finding| {
                finding.rule_id.as_deref() == core_id_for(RULE_NAMESPACE_OWNERSHIP_VIOLATION)
                    && snippet(finding).contains("rules owner 'omnia' may only use")
                    && snippet(finding).contains("OMNIA-*")
                    && snippet(finding).contains("CORE-001")
            }),
            "expected CORE-* under target adapter to be rejected, got: {findings:?}",
        );
    }

    #[test]
    fn core_rule_under_source_adapter_rejected() {
        let temp = TempDir::new().expect("tempdir");
        scaffold_framework(temp.path());
        write_rule(
            temp.path(),
            "adapters/sources/documentation/rules/core-misplaced.md",
            "CORE-007",
        );

        let findings = run_rules_check(&ctx_for(temp.path()));
        assert!(
            findings.iter().any(|finding| {
                finding.rule_id.as_deref() == core_id_for(RULE_NAMESPACE_OWNERSHIP_VIOLATION)
                    && snippet(finding).contains("rules owner 'documentation' may only use")
                    && snippet(finding).contains("SRC-*")
                    && snippet(finding).contains("CORE-007")
            }),
            "expected CORE-* under source adapter to be rejected, got: {findings:?}",
        );
    }

    #[test]
    fn non_core_rule_under_core_pack_rejected() {
        let temp = TempDir::new().expect("tempdir");
        scaffold_framework(temp.path());
        write_rule(temp.path(), "adapters/shared/rules/core/foreign.md", "UNI-001");

        let findings = run_rules_check(&ctx_for(temp.path()));
        assert!(
            findings.iter().any(|finding| {
                finding.rule_id.as_deref() == core_id_for(RULE_NAMESPACE_OWNERSHIP_VIOLATION)
                    && snippet(finding).contains("rules owner 'core' may only use")
                    && snippet(finding).contains("CORE-*")
                    && snippet(finding).contains("UNI-001")
            }),
            "expected non-CORE-* under core pack to be rejected, got: {findings:?}",
        );
    }

    #[test]
    fn vectis_overlay_rust_id_rejected() {
        let temp = TempDir::new().expect("tempdir");
        scaffold_framework(temp.path());
        write_rule(temp.path(), "adapters/targets/vectis/rules/rust-misplaced.md", "RUST-001");

        let findings = run_rules_check(&ctx_for(temp.path()));
        assert!(
            findings.iter().any(|finding| {
                finding.rule_id.as_deref() == core_id_for(RULE_NAMESPACE_OWNERSHIP_VIOLATION)
                    && snippet(finding).contains("rules owner 'vectis' may only use")
                    && snippet(finding).contains("VECTIS-*")
                    && snippet(finding).contains("RUST-001")
            }),
            "expected vectis to keep rejecting non-VECTIS ids, got: {findings:?}",
        );
    }
}
