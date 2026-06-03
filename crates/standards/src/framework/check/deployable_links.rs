use std::fs;
use std::path::Path;

use regex::Regex;
use specify_diagnostics::Diagnostic;
use walkdir::WalkDir;

use crate::framework::builder::{framework_finding, loc};
use crate::framework::check::Check;
use crate::framework::context::Context;
use crate::framework::helpers::{relative_display, strip_html_comments};

const RULE_DOCS_IN_DEPLOYABLE: &str = "links.docs-in-deployable-surface";

/// Forbid root-relative escapes into `docs/` from plugin and adapter deploy surfaces.
pub struct DeployableLinksCheck;

impl Check for DeployableLinksCheck {
    fn run(&self, ctx: &Context) -> Vec<Diagnostic> {
        run(ctx)
    }
}

fn run(ctx: &Context) -> Vec<Diagnostic> {
    let root = ctx.framework_root();
    let link_re = link_pattern();
    let mut findings = Vec::new();

    for path in deployable_markdown_files(root) {
        let rel = relative_display(root, &path);
        let content = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(_) => continue,
        };
        let stripped = strip_html_comments(&content);
        for cap in link_re.captures_iter(&stripped) {
            let target = cap.get(1).map(|m| m.as_str()).unwrap_or("");
            if is_url_scheme(target) {
                continue;
            }
            if target.contains("../docs/") || target.starts_with("docs/") {
                findings.push(framework_finding(
                    RULE_DOCS_IN_DEPLOYABLE,
                    format!(
                        "Deployable link targets docs/: {rel} links to '{target}' — use plugins/spec/references/ or ../references/spec-runtime/ (or https://specify.augentic.io for optional depth)"
                    ),
                    Some(loc(path.clone(), 1, None)),
                ));
            }
        }
    }

    findings
}

fn deployable_markdown_files(root: &Path) -> Vec<std::path::PathBuf> {
    let mut paths = Vec::new();
    for base in ["plugins", "adapters"] {
        let base_path = root.join(base);
        if !base_path.is_dir() {
            continue;
        }
        for entry in WalkDir::new(&base_path).follow_links(false).into_iter().flatten() {
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path().to_path_buf();
            let rel = path.strip_prefix(root).unwrap_or(&path);
            let rel_str = rel.to_string_lossy();
            if !rel_str.ends_with(".md") {
                continue;
            }
            if rel_str.contains("/adapters/shared/rules/") {
                continue;
            }
            if rel_str.contains("/adapters/shared/references/runtime/") {
                continue;
            }
            if rel_str.contains("/briefs/")
                || rel_str.contains("/references/")
                || rel_str.starts_with("plugins/")
            {
                paths.push(path);
            }
        }
    }
    paths.sort();
    paths
}

fn is_url_scheme(target: &str) -> bool {
    let Some(colon) = target.find("://") else {
        return false;
    };
    let scheme = &target[..colon];
    !scheme.is_empty()
        && scheme.chars().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || c == '+' || c == '-' || c == '.'
        })
}

fn link_pattern() -> &'static Regex {
    use std::sync::OnceLock;
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\[[^\]]*\]\(([^)]+)\)").expect("valid markdown link pattern"))
}
