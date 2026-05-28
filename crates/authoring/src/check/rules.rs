use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

use crate::context::Context;
use crate::finding::{Check, Finding, Location};
use crate::helpers::{relative_display, skill_frontmatter, under_symlink, walk_matching_files};
use crate::schema::{SchemaError, SchemaId, ValidationError, validate_frontmatter};

pub const RULE_SCHEMA_VIOLATION: &str = "codex.schema-violation";
pub const RULE_NAMESPACE_OWNERSHIP_VIOLATION: &str = "codex.namespace-ownership-violation";
pub const RULE_DUPLICATE_RULE_ID: &str = "codex.duplicate-rule-id";

const SHARED_CODEX_OWNER: &str = "universal";

static CODEX_RULE_HEADING_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^## Rule\s*$").expect("codex rule heading regex"));

static RULE_ID_NAMESPACE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([A-Z]+)-[0-9]{3}$").expect("codex rule id regex"));

/// Static (target + shared) namespace map. Source-axis owners are discovered
/// per-run because every adapter under `adapters/sources/<name>/codex/` owns
/// the shared `SRC-*` namespace, and we refuse to hardcode source-adapter
/// names. See [`codex_profile_namespaces`].
static STATIC_CODEX_PROFILE_NAMESPACES: LazyLock<HashMap<&'static str, HashSet<&'static str>>> =
    LazyLock::new(|| {
        HashMap::from([
            (SHARED_CODEX_OWNER, HashSet::from(["UNI"])),
            ("omnia", HashSet::from(["OMNIA", "RUST", "SEC"])),
            ("contracts", HashSet::from(["IFACE"])),
            ("vectis", HashSet::from(["VECTIS"])),
        ])
    });

/// Codex rule shape validation and RFC-28 namespace ownership.
pub struct CodexCheck;

impl Check for CodexCheck {
    fn run(&self, ctx: &Context) -> Vec<Finding> {
        run_codex_check(ctx)
    }
}

pub fn run_codex_check(ctx: &Context) -> Vec<Finding> {
    let paths = match discover_codex_rule_files(ctx) {
        Ok(paths) => paths,
        Err(error) => {
            return vec![Finding {
                rule_id: RULE_SCHEMA_VIOLATION,
                message: format!("Codex rule discovery failed: {error}"),
                location: None,
            }];
        }
    };

    let profile_namespaces = codex_profile_namespaces(ctx);
    let mut findings = Vec::new();
    let mut ids_by_value: BTreeMap<String, Vec<String>> = BTreeMap::new();

    for path in paths {
        let rel = relative_display(ctx.framework_root(), &path);
        let content = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(source) => {
                findings.push(finding_at(
                    RULE_SCHEMA_VIOLATION,
                    format!("Codex rule: {rel} — cannot read: {source}"),
                    &path,
                ));
                continue;
            }
        };

        match validate_frontmatter(ctx, &path, SchemaId::CodexRule) {
            Ok(()) => {}
            Err(SchemaError::Infrastructure(error)) => {
                findings.push(finding_at(
                    RULE_SCHEMA_VIOLATION,
                    format!("Codex rule: {rel} — {error}"),
                    &path,
                ));
            }
            Err(SchemaError::Validation(errors)) => {
                for error in errors {
                    let detail = format_validation_error(&error);
                    let prefix = if error.message.contains("missing leading YAML frontmatter") {
                        "Codex rule"
                    } else {
                        "Codex rule frontmatter"
                    };
                    findings.push(finding_at(
                        RULE_SCHEMA_VIOLATION,
                        format!("{prefix}: {rel} — {detail}"),
                        &path,
                    ));
                }
            }
        }

        if let Some(body) = codex_body(&content)
            && !CODEX_RULE_HEADING_RE.is_match(body)
        {
            findings.push(finding_at(
                RULE_SCHEMA_VIOLATION,
                format!("Codex rule body: {rel} — missing required '## Rule' heading"),
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
            && owner != SHARED_CODEX_OWNER
        {
            findings.push(finding_at(
                RULE_NAMESPACE_OWNERSHIP_VIOLATION,
                format!(
                    "Codex namespace ownership: {rel} — FRAME-* ids are reserved by RFC-32 for framework-repo declarative rules and may not be placed under adapter trees (got '{id}' under codex owner '{owner}')"
                ),
                &path,
            ));
            continue;
        }

        let Some(allowed_namespaces) = profile_namespaces.get(owner.as_str()) else {
            findings.push(finding_at(
                RULE_NAMESPACE_OWNERSHIP_VIOLATION,
                format!(
                    "Codex namespace ownership: {rel} — codex owner '{owner}' has no configured namespace; update crates/authoring/src/check/codex.rs before adding first-party rules here"
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
                    "Codex namespace ownership: {rel} — codex owner '{owner}' may only use {} ids, got '{id}'",
                    namespace_list(allowed_namespaces)
                ),
                &path,
            ));
        }
    }

    for (id, paths) in ids_by_value {
        if paths.len() > 1 {
            findings.push(Finding {
                rule_id: RULE_DUPLICATE_RULE_ID,
                message: format!(
                    "Codex rule duplicate id '{id}' across files: {}",
                    paths.join(", ")
                ),
                location: None,
            });
        }
    }

    findings
}

/// Build the codex-owner → allowed-namespaces map for this run.
///
/// Target adapters and the shared `universal` owner come from the static base
/// map. Source adapters are discovered dynamically: every directory under
/// `adapters/sources/<name>/codex/` registers `<name>` → `{"SRC"}`. RFC-28
/// §Namespaces forbids hardcoding source-adapter names here — `SRC-*` is the
/// single shared source-axis namespace in v1.
fn codex_profile_namespaces(ctx: &Context) -> HashMap<String, HashSet<&'static str>> {
    let mut profile: HashMap<String, HashSet<&'static str>> = STATIC_CODEX_PROFILE_NAMESPACES
        .iter()
        .map(|(owner, namespaces)| ((*owner).to_string(), namespaces.clone()))
        .collect();

    for owner in source_codex_owners(ctx) {
        profile.entry(owner).or_insert_with(|| HashSet::from(["SRC"]));
    }

    profile
}

/// Discover source-adapter owners that contribute a codex overlay.
///
/// Returns the first-segment directory name (e.g. `documentation`) for every
/// `adapters/sources/<name>/` directory that contains a `codex/` subdirectory.
/// Matches the placement predicate in `is_codex_rule_in_axis`: an owner is
/// only considered to contribute to the namespace map when a `codex/` subtree
/// exists for it, mirroring how `discover_codex_rule_files` only yields rules
/// from such trees.
fn source_codex_owners(ctx: &Context) -> Vec<String> {
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
        if !entry.path().join("codex").is_dir() {
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

fn discover_codex_rule_files(ctx: &Context) -> Result<Vec<PathBuf>, crate::error::ToolingError> {
    let framework_root = ctx.framework_root();
    let mut paths = Vec::new();

    for axis_dir in [ctx.sources_dir(), ctx.targets_dir()] {
        let files = walk_matching_files(framework_root, &axis_dir, ".md")?;
        for path in files {
            if is_codex_readme(&path) {
                continue;
            }
            if is_codex_rule_in_axis(&path, &axis_dir) {
                paths.push(path);
            }
        }
    }

    let shared_dir = ctx.adapters_shared_dir().join("codex").join(SHARED_CODEX_OWNER);
    if shared_dir.is_dir() {
        let files = walk_matching_files(framework_root, &shared_dir, ".md")?;
        for path in files {
            if is_codex_readme(&path) {
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

fn is_codex_readme(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("readme.md"))
}

fn is_codex_rule_in_axis(path: &Path, axis_root: &Path) -> bool {
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
    parts.len() >= 3 && parts.get(1) == Some(&"codex")
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
            if parts.len() >= 3 && parts.get(1) == Some(&"codex") {
                return parts.first().map(|part| (*part).to_string());
            }
        }
    }

    let shared_dir = ctx.adapters_shared_dir().join("codex").join(SHARED_CODEX_OWNER);
    if path.strip_prefix(&shared_dir).is_ok() {
        return Some(SHARED_CODEX_OWNER.to_string());
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

fn codex_body(content: &str) -> Option<&str> {
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

fn finding_at(rule_id: &'static str, message: String, path: &Path) -> Finding {
    Finding {
        rule_id,
        message,
        location: Some(Location {
            path: path.to_path_buf(),
            line: 1,
            column: None,
        }),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use tempfile::TempDir;

    use super::*;

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
            "---\nid: {id}\ntitle: Test Rule\nseverity: important\ntrigger: When testing codex validation in specdev check.\n---\n\n## Rule\n\nBody.\n"
        );
        fs::write(path, body).expect("write rule");
    }

    fn ctx_for(root: &Path) -> Context {
        Context::from_framework_root(root).expect("framework root")
    }

    #[test]
    fn codex_profile_namespaces_registers_source_owners_dynamically() {
        let temp = TempDir::new().expect("tempdir");
        scaffold_framework(temp.path());
        fs::create_dir_all(temp.path().join("adapters/sources/documentation/codex"))
            .expect("documentation codex");
        fs::create_dir_all(temp.path().join("adapters/sources/captures/codex"))
            .expect("captures codex");
        fs::create_dir_all(temp.path().join("adapters/sources/intent")).expect("intent no codex");

        let ctx = ctx_for(temp.path());
        let profile = codex_profile_namespaces(&ctx);

        assert_eq!(profile.get("documentation"), Some(&HashSet::from(["SRC"])));
        assert_eq!(profile.get("captures"), Some(&HashSet::from(["SRC"])));
        assert!(
            !profile.contains_key("intent"),
            "intent has no codex/ subtree so it must not be registered",
        );
        assert_eq!(profile.get(SHARED_CODEX_OWNER), Some(&HashSet::from(["UNI"])));
        assert_eq!(profile.get("omnia"), Some(&HashSet::from(["OMNIA", "RUST", "SEC"])));
        assert_eq!(profile.get("vectis"), Some(&HashSet::from(["VECTIS"])));
        assert_eq!(profile.get("contracts"), Some(&HashSet::from(["IFACE"])));
    }

    #[test]
    fn src_rule_under_source_adapter_passes_namespace_check() {
        let temp = TempDir::new().expect("tempdir");
        scaffold_framework(temp.path());
        write_rule(
            temp.path(),
            "adapters/sources/documentation/codex/source-overlay.md",
            "SRC-001",
        );

        let findings = run_codex_check(&ctx_for(temp.path()));
        let ownership: Vec<_> = findings
            .iter()
            .filter(|finding| finding.rule_id == RULE_NAMESPACE_OWNERSHIP_VIOLATION)
            .collect();
        assert!(
            ownership.is_empty(),
            "SRC-* under source-adapter codex should pass, got: {ownership:?}",
        );
    }

    #[test]
    fn non_src_rule_under_source_adapter_rejected() {
        let temp = TempDir::new().expect("tempdir");
        scaffold_framework(temp.path());
        write_rule(
            temp.path(),
            "adapters/sources/documentation/codex/wrong-namespace.md",
            "OMNIA-001",
        );

        let findings = run_codex_check(&ctx_for(temp.path()));
        assert!(
            findings.iter().any(|finding| {
                finding.rule_id == RULE_NAMESPACE_OWNERSHIP_VIOLATION
                    && finding.message.contains("codex owner 'documentation' may only use")
                    && finding.message.contains("SRC-*")
                    && finding.message.contains("OMNIA-001")
            }),
            "expected SRC-only enforcement under source adapter, got: {findings:?}",
        );
    }

    #[test]
    fn frame_rule_under_target_adapter_rejected_with_rfc32_message() {
        let temp = TempDir::new().expect("tempdir");
        scaffold_framework(temp.path());
        write_rule(temp.path(), "adapters/targets/omnia/codex/frame-misplaced.md", "FRAME-001");

        let findings = run_codex_check(&ctx_for(temp.path()));
        assert!(
            findings.iter().any(|finding| {
                finding.rule_id == RULE_NAMESPACE_OWNERSHIP_VIOLATION
                    && finding.message.contains("FRAME-*")
                    && finding.message.contains("RFC-32")
                    && finding.message.contains("FRAME-001")
                    && finding.message.contains("omnia")
            }),
            "expected FRAME placement violation with RFC-32 reservation message, got: {findings:?}",
        );
    }

    #[test]
    fn frame_rule_under_source_adapter_rejected_with_rfc32_message() {
        let temp = TempDir::new().expect("tempdir");
        scaffold_framework(temp.path());
        write_rule(
            temp.path(),
            "adapters/sources/documentation/codex/frame-misplaced.md",
            "FRAME-007",
        );

        let findings = run_codex_check(&ctx_for(temp.path()));
        assert!(
            findings.iter().any(|finding| {
                finding.rule_id == RULE_NAMESPACE_OWNERSHIP_VIOLATION
                    && finding.message.contains("FRAME-*")
                    && finding.message.contains("RFC-32")
                    && finding.message.contains("FRAME-007")
                    && finding.message.contains("documentation")
            }),
            "expected FRAME placement violation under source adapter, got: {findings:?}",
        );
    }

    #[test]
    fn target_overlay_rust_id_under_vectis_still_rejected() {
        let temp = TempDir::new().expect("tempdir");
        scaffold_framework(temp.path());
        write_rule(temp.path(), "adapters/targets/vectis/codex/rust-misplaced.md", "RUST-001");

        let findings = run_codex_check(&ctx_for(temp.path()));
        assert!(
            findings.iter().any(|finding| {
                finding.rule_id == RULE_NAMESPACE_OWNERSHIP_VIOLATION
                    && finding.message.contains("codex owner 'vectis' may only use")
                    && finding.message.contains("VECTIS-*")
                    && finding.message.contains("RUST-001")
            }),
            "expected vectis to keep rejecting non-VECTIS ids, got: {findings:?}",
        );
    }
}
