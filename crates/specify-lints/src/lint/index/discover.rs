//! Rules tree discovery per the `WorkspaceModel` entity families and
//! rules-root resolution.
//!
//! For the consumer scan, the rules tree lives at
//! `<project_dir>/.specify/cache/rules/` (populated by `specrun init`
//! and refreshed by `specrun workspace sync`). This module walks that
//! tree, reuses the rule frontmatter parser
//! ([`crate::rules::parse::parse_rule_file`]), and emits one
//! [`RuleIndexEntry`] per `*.md` rule found.
//!
//! Origin inference is by path shape, mirroring the resolver's
//! overlay precedence in [`mod@crate::rules::resolve`]:
//!
//! - `adapters/targets/<name>/rules/…` → [`Origin::Target`]
//! - `adapters/sources/<name>/rules/…` → [`Origin::Source`]
//! - `adapters/shared/rules/universal/…` → [`Origin::Shared`]
//! - anything else under the cache → [`Origin::Organization`]
//!
//! `frontmatter_ref` is the project-relative path with a stable
//! `#frontmatter` anchor appended. For rules the canonical
//! cross-reference handle is the `rule_index` family itself; the
//! `frontmatter` extractor only fires on markdown files outside the
//! cache so the two surfaces do not double-count the same file.
//!
//! Parse failures collapse to per-file skips silently in v1; the reserved-hint diagnostic surface
//! reserved-hint diagnostics reserves the `index.warning` finding for S7's hint runner.
//! Missing or absent rules trees return an empty vector rather than
//! erroring.

use std::path::Path;

use ignore::WalkBuilder;

use crate::lint::RuleIndexEntry;
use crate::rules::Origin;
use crate::rules::parse::parse_rule_file;

const CACHE_ROOT: &str = ".specify/cache/rules";

/// Walk the consumer rules cache and emit one fact per discovered
/// `*.md` rule. Returns an empty vector when the cache is missing.
#[must_use]
pub fn discover(project_dir: &Path) -> Vec<RuleIndexEntry> {
    let root = project_dir.join(CACHE_ROOT);
    if !root.is_dir() {
        return Vec::new();
    }

    let mut facts: Vec<RuleIndexEntry> = Vec::new();
    for entry in
        WalkBuilder::new(&root).follow_links(false).standard_filters(false).build().flatten()
    {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
            continue;
        }
        let Ok(relative) = path.strip_prefix(project_dir) else {
            continue;
        };
        let Some(relative_str) = relative.to_str() else {
            continue;
        };
        let relative_str = relative_str.replace(std::path::MAIN_SEPARATOR, "/");
        let Ok(rule) = parse_rule_file(path) else {
            continue;
        };
        let cache_relative =
            relative_str.strip_prefix(&format!("{CACHE_ROOT}/")).unwrap_or(relative_str.as_str());
        let origin = infer_origin(cache_relative);
        let frontmatter_ref = format!("{relative_str}#frontmatter");
        facts.push(RuleIndexEntry {
            rule_id: rule.id,
            path: relative_str,
            origin,
            frontmatter_ref,
        });
    }
    facts.sort_by(|a, b| a.rule_id.cmp(&b.rule_id));
    facts
}

fn infer_origin(cache_relative: &str) -> Origin {
    if let Some(rest) = cache_relative.strip_prefix("adapters/") {
        if let Some(rest) = rest.strip_prefix("targets/")
            && rest.contains("/rules/")
        {
            return Origin::Target;
        }
        if let Some(rest) = rest.strip_prefix("sources/")
            && rest.contains("/rules/")
        {
            return Origin::Source;
        }
        if rest.starts_with("shared/rules/universal/") {
            return Origin::Shared;
        }
    }
    Origin::Organization
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_inference_matches_overlay_precedence() {
        assert_eq!(infer_origin("adapters/shared/rules/universal/UNI-014.md"), Origin::Shared);
        assert_eq!(infer_origin("adapters/targets/omnia/rules/OMNIA-001.md"), Origin::Target,);
        assert_eq!(infer_origin("adapters/sources/documentation/rules/SRC-001.md"), Origin::Source,);
        assert_eq!(infer_origin("organization/local-policy.md"), Origin::Organization);
    }
}
