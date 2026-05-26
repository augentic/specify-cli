use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;

use regex::Regex;
use walkdir::WalkDir;

use crate::context::Context;
use crate::finding::{Check, Finding, Location};
use crate::helpers::under_symlink;

const RULE_BRIEF_SCHEMA_LINK_RESOLVE: &str = "links.brief-schema-link-resolve";

/// Known tool → schema-name registry. The contract tool declares no
/// embedded schemas; vectis owns three.
const KNOWN_SCHEMAS: &[(&str, &[&str])] = &[("vectis", &["tokens", "assets", "composition"])];

/// Validate that `schemas.specify.dev` URLs in adapter briefs and
/// references resolve to a known tool-owned schema.
pub struct SchemaLinksCheck;

impl Check for SchemaLinksCheck {
    fn run(&self, ctx: &Context) -> Vec<Finding> {
        run_on_root(ctx.framework_root())
    }
}

/// Run the schema-link predicate against a framework root (used by
/// integration tests).
pub fn run_on_root(root: &std::path::Path) -> Vec<Finding> {
    let url_re = schema_url_pattern();
    let fence_re = fenced_code_pattern();
    let inline_re = inline_code_pattern();

    let mut findings = Vec::new();

    let adapters_dir = root.join("adapters");
    if !adapters_dir.is_dir() {
        return findings;
    }

    for path in walk_adapter_markdown(root, &adapters_dir).unwrap_or_default() {
        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let stripped = {
            let no_fence = fence_re.replace_all(&content, "");
            inline_re.replace_all(&no_fence, "").into_owned()
        };

        let rel = path.strip_prefix(root).unwrap_or(&path).to_string_lossy().replace('\\', "/");

        for (line_idx, line) in stripped.lines().enumerate() {
            for cap in url_re.captures_iter(line) {
                let tool = cap.get(1).map(|m| m.as_str()).unwrap_or("");
                let name_with_ext = cap.get(2).map(|m| m.as_str()).unwrap_or("");
                let name = name_with_ext.strip_suffix(".schema.json").unwrap_or(name_with_ext);

                if !is_known_schema(tool, name) {
                    let url = cap.get(0).map(|m| m.as_str()).unwrap_or("");
                    findings.push(Finding {
                        rule_id: RULE_BRIEF_SCHEMA_LINK_RESOLVE,
                        message: format!(
                            "{rel}:{} — schema URL '{url}' does not resolve to a known \
                             tool-owned schema",
                            line_idx + 1,
                        ),
                        location: Some(Location {
                            path: path.clone(),
                            line: line_idx + 1,
                            column: None,
                        }),
                    });
                }
            }
        }
    }

    findings
}

fn is_known_schema(tool: &str, name: &str) -> bool {
    KNOWN_SCHEMAS.iter().any(|(t, names)| *t == tool && names.contains(&name))
}

fn walk_adapter_markdown(
    framework_root: &std::path::Path, adapters_dir: &std::path::Path,
) -> Result<Vec<PathBuf>, crate::error::ToolingError> {
    let mut out = Vec::new();
    for entry in WalkDir::new(adapters_dir).follow_links(false).into_iter() {
        let entry = entry.map_err(|source| {
            crate::error::ToolingError::Infrastructure(format!(
                "walk {}: {source}",
                adapters_dir.display()
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
        out.push(path);
    }
    out.sort();
    Ok(out)
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

fn fenced_code_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"```[\s\S]*?```").expect("valid fence pattern"))
}

fn inline_code_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"`[^`]+`").expect("valid inline code pattern"))
}
