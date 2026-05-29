use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;
use serde_json::Value;
use walkdir::WalkDir;

use crate::framework::builder::{framework_finding, loc};
use crate::framework::check::Check;
use crate::framework::context::Context;
use crate::framework::helpers::under_symlink;
use crate::rules::Diagnostic;

const RULE_INVALID_DECLARATION: &str = "tools.invalid-declaration";
const RULE_INVOCATION_NOT_EQUIVALENT: &str = "tools.invocation-not-equivalent";

struct ExpectedToolDeclaration {
    adapter: &'static str,
    name: &'static str,
    package: &'static str,
}

const EXPECTED_FIRST_PARTY_TOOLS: &[ExpectedToolDeclaration] = &[
    ExpectedToolDeclaration {
        adapter: "contracts",
        name: "contract",
        package: "specify:contract@0.3.0",
    },
    ExpectedToolDeclaration {
        adapter: "vectis",
        name: "vectis",
        package: "specify:vectis@0.3.0",
    },
];

#[derive(Copy, Clone)]
struct RetiredHelperPattern {
    token: &'static str,
    pattern: &'static str,
    replacement: &'static str,
}

const RETIRED_HELPER_PATTERNS: &[RetiredHelperPattern] = &[
    RetiredHelperPattern {
        token: "specify-contract-validate",
        pattern: r"\bspecify-contract-validate\b",
        replacement: "specrun tool run contract -- <BASELINE_DIR> --format json",
    },
    RetiredHelperPattern {
        token: "specify-contract",
        pattern: r"\bspecify-contract\b",
        replacement: "specrun tool run contract -- <BASELINE_DIR> --format json",
    },
    RetiredHelperPattern {
        token: "specify-vectis validate",
        pattern: r"\bspecify-vectis\s+validate\b",
        replacement: "specrun tool run vectis -- validate <mode> [path]",
    },
    RetiredHelperPattern {
        token: "specify vectis validate",
        pattern: r"\bspecify\s+vectis\s+validate\b",
        replacement: "specrun tool run vectis -- validate <mode> [path]",
    },
    RetiredHelperPattern {
        token: "specify-vectis init",
        pattern: r"\bspecify-vectis\s+init\b",
        replacement: "specrun tool run vectis -- scaffold core <app-name>",
    },
    RetiredHelperPattern {
        token: "specify vectis init",
        pattern: r"\bspecify\s+vectis\s+init\b",
        replacement: "specrun tool run vectis -- scaffold core <app-name>",
    },
    RetiredHelperPattern {
        token: "specify-vectis add-shell",
        pattern: r"\bspecify-vectis\s+add-shell\b",
        replacement: "specrun tool run vectis -- scaffold ios|android <app-name>",
    },
    RetiredHelperPattern {
        token: "specify vectis add-shell",
        pattern: r"\bspecify\s+vectis\s+add-shell\b",
        replacement: "specrun tool run vectis -- scaffold ios|android <app-name>",
    },
];

fn retired_helper_regexes() -> &'static [(&'static RetiredHelperPattern, Regex)] {
    static CACHE: OnceLock<Vec<(&'static RetiredHelperPattern, Regex)>> = OnceLock::new();
    CACHE.get_or_init(|| {
        RETIRED_HELPER_PATTERNS
            .iter()
            .map(|helper| {
                let regex = Regex::new(helper.pattern).unwrap_or_else(|error| {
                    panic!("retired helper regex {}: {error}", helper.token)
                });
                (helper, regex)
            })
            .collect()
    })
}

/// Validate first-party WASM tool declarations in target adapter manifests.
pub struct FirstPartyTools;

impl Check for FirstPartyTools {
    fn run(&self, ctx: &Context) -> Vec<Diagnostic> {
        check_first_party_tools(ctx)
    }
}

/// Reject retired host helper invocations that have declared-tool equivalents.
pub struct DeclaredToolInvocations;

impl Check for DeclaredToolInvocations {
    fn run(&self, ctx: &Context) -> Vec<Diagnostic> {
        check_declared_tool_invocations(ctx)
    }
}

/// Run first-party tool declaration validation against `ctx`.
pub fn check_first_party_tools(ctx: &Context) -> Vec<Diagnostic> {
    let mut findings = Vec::new();
    let mut cache: HashMap<String, Option<ResolvedAdapter>> = HashMap::new();
    let mut shape_reported = HashSet::new();

    for expected in EXPECTED_FIRST_PARTY_TOOLS {
        let resolved = cache
            .entry(expected.adapter.to_string())
            .or_insert_with(|| resolve_adapter_declarations(ctx, expected.adapter))
            .clone();

        let Some(resolved) = resolved else {
            continue;
        };

        if shape_reported.insert(expected.adapter.to_string()) {
            findings.extend(resolved.shape_findings);
        }

        let package_request = resolved.declarations.get(expected.name);
        match package_request {
            None => findings.push(invalid_declaration(
                &resolved.rel,
                &resolved.path,
                &format!("missing tool '{}'", expected.name),
            )),
            Some(package) if package != expected.package => findings.push(invalid_declaration(
                &resolved.rel,
                &resolved.path,
                &format!("'{}' package must be '{}'", expected.name, expected.package),
            )),
            _ => {}
        }
    }

    findings
}

/// Run declared-tool invocation equivalence validation against `ctx`.
pub fn check_declared_tool_invocations(ctx: &Context) -> Vec<Diagnostic> {
    let mut findings = Vec::new();
    let root = ctx.framework_root();

    let Ok(files) = active_brief_and_skill_files(ctx) else {
        return findings;
    };

    for path in files {
        let rel = path.strip_prefix(root).unwrap_or(&path).to_string_lossy().replace('\\', "/");
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };

        for (line_idx, line) in content.lines().enumerate() {
            for (helper, pattern) in retired_helper_regexes() {
                if !retired_helper_matches(line, helper, pattern) {
                    continue;
                }
                findings.push(framework_finding(
                    RULE_INVOCATION_NOT_EQUIVALENT,
                    format!(
                        "{}:{} — '{}' has a declared-tool equivalent; use `{}`",
                        rel,
                        line_idx + 1,
                        helper.token,
                        helper.replacement
                    ),
                    Some(loc(PathBuf::from(&rel), line_idx + 1, None)),
                ));
            }
        }
    }

    findings
}

fn retired_helper_matches(line: &str, helper: &RetiredHelperPattern, pattern: &Regex) -> bool {
    if helper.token != "specify-contract" {
        return pattern.is_match(line);
    }

    pattern.find_iter(line).any(|m| !line[m.end()..].starts_with("-validate"))
}

#[derive(Clone)]
struct ResolvedAdapter {
    rel: String,
    path: PathBuf,
    declarations: HashMap<String, String>,
    shape_findings: Vec<Diagnostic>,
}

fn resolve_adapter_declarations(ctx: &Context, adapter: &str) -> Option<ResolvedAdapter> {
    let path = ctx.targets_dir().join(adapter).join("adapter.yaml");
    if !path.is_file() {
        return None;
    }

    let rel = path
        .strip_prefix(ctx.framework_root())
        .unwrap_or(&path)
        .to_string_lossy()
        .replace('\\', "/");
    let raw = fs::read_to_string(&path).ok()?;
    let manifest: Value = serde_saphyr::from_str(&raw).ok()?;
    let tools =
        manifest.get("tools").and_then(|value| value.as_array()).cloned().unwrap_or_default();

    let mut shape_findings = Vec::new();
    let mut declarations = HashMap::new();

    for tool in tools {
        let Some(entry) = tool.as_object() else {
            shape_findings.push(invalid_declaration(
                &rel,
                &path,
                "`tools[]` entries must be { name, version } objects under target.schema.json",
            ));
            continue;
        };

        let name = entry.get("name").and_then(|value| value.as_str());
        let version = entry.get("version").and_then(|value| value.as_str());
        let (Some(name), Some(version)) = (name, version) else {
            shape_findings.push(invalid_declaration(
                &rel,
                &path,
                "tool object must carry string `name` and `version` fields",
            ));
            continue;
        };

        declarations.insert(name.to_string(), format!("specify:{name}@{version}"));
    }

    Some(ResolvedAdapter {
        rel,
        path,
        declarations,
        shape_findings,
    })
}

fn invalid_declaration(rel: &str, path: &Path, detail: &str) -> Diagnostic {
    framework_finding(
        RULE_INVALID_DECLARATION,
        format!("First-party tool declaration: {rel} — {detail}"),
        Some(loc(path, 1, None)),
    )
}

fn active_brief_and_skill_files(
    ctx: &Context,
) -> Result<Vec<PathBuf>, crate::framework::error::ToolingError> {
    let mut files = Vec::new();
    let root = ctx.framework_root();
    let targets_dir = ctx.targets_dir();

    if targets_dir.is_dir() {
        collect_markdown_under(
            root,
            &targets_dir,
            |rel_parts| rel_parts.len() >= 3 && rel_parts[1] == "briefs",
            &mut files,
        )?;
    }

    let plugins_dir = ctx.plugins_dir();
    if plugins_dir.is_dir() {
        collect_markdown_under(
            root,
            &plugins_dir,
            |rel_parts| rel_parts.len() >= 3 && rel_parts[1] == "skills",
            &mut files,
        )?;
    }

    files.sort();
    Ok(files)
}

fn collect_markdown_under(
    framework_root: &Path, root: &Path, include: impl Fn(&[&str]) -> bool, out: &mut Vec<PathBuf>,
) -> Result<(), crate::framework::error::ToolingError> {
    for entry in WalkDir::new(root).follow_links(false).into_iter() {
        let entry = entry.map_err(|source| {
            crate::framework::error::ToolingError::Infrastructure(format!(
                "walk {}: {source}",
                root.display()
            ))
        })?;
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.into_path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
            continue;
        }
        if under_symlink(framework_root, &path)? {
            continue;
        }
        let rel = path.strip_prefix(root).unwrap_or(&path);
        let rel_parts: Vec<&str> =
            rel.components().filter_map(|component| component.as_os_str().to_str()).collect();
        if include(&rel_parts) {
            out.push(path);
        }
    }
    Ok(())
}
