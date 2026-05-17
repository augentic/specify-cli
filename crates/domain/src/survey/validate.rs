//! Semantic validators for `SurfacesDocument` and `MetadataDocument`.
//!
//! JSON-schema validation catches structural errors (handled by callers
//! via `validate_against_schema`). The functions here enforce invariants
//! the schema cannot express: sorted lists, non-empty `declared-at`,
//! no out-of-tree paths (absolute, Windows-drive, or `..`-traversing),
//! and no duplicate surface ids.
//!
//! "No timestamps" and "no host-state leaks" are enforced by schema
//! closure (`additionalProperties: false`) plus the out-of-tree check
//! below. No extra code is needed for those guardrails.

use std::collections::HashSet;

use specify_error::Error;

use super::dto::{MetadataDocument, SurfacesDocument};

/// Rule id for any `touches[]` / `declared-at[]` entry that is not a
/// relative path under the source root: absolute, Windows-drive-prefixed,
/// `..`-traversing, or (when checked on disk) escaping the canonical
/// source root.
pub const RULE_TOUCHES_OUT_OF_TREE: &str = "surfaces-touches-out-of-tree";

/// Validate semantic invariants on a `SurfacesDocument`.
///
/// # Errors
///
/// Returns `Error::Validation` with one finding per violated invariant.
pub fn validate_surfaces(doc: &SurfacesDocument) -> Result<(), Error> {
    let mut findings = Vec::new();

    if doc.version != 1 {
        findings.push(finding(
            "surfaces-version-unsupported",
            "surfaces.json version must be 1",
            format!("found version {}", doc.version),
        ));
    }

    if !doc.surfaces.windows(2).all(|w| w[0].id <= w[1].id) {
        findings.push(finding(
            "surfaces-out-of-order",
            "surfaces[] must be sorted by id",
            "entries are not in ascending id order".to_string(),
        ));
    }

    let mut seen_ids = HashSet::new();
    for (i, s) in doc.surfaces.iter().enumerate() {
        if !seen_ids.insert(&s.id) {
            findings.push(finding(
                "surface-id-duplicate",
                "surfaces[].id must be unique",
                format!("duplicate id: {}", s.id),
            ));
        }

        if !s.touches.windows(2).all(|w| w[0] <= w[1]) {
            findings.push(finding(
                "surfaces-touches-out-of-order",
                "surfaces[].touches must be sorted alphabetically",
                format!("touches out of order in surface `{}`", s.id),
            ));
        }

        if s.declared_at.is_empty() {
            findings.push(finding(
                "surfaces-declared-at-empty",
                "surfaces[].declared-at must be non-empty",
                format!("declared-at is empty for surface `{}`", s.id),
            ));
        }

        if !s.declared_at.windows(2).all(|w| w[0] <= w[1]) {
            findings.push(finding(
                "surfaces-declared-at-out-of-order",
                "surfaces[].declared-at must be sorted alphabetically",
                format!("declared-at out of order in surface `{}`", s.id),
            ));
        }

        for (j, p) in s.touches.iter().enumerate() {
            check_out_of_tree(p, &format!("surfaces[{i}].touches[{j}]"), false, &mut findings);
        }
        for (j, p) in s.declared_at.iter().enumerate() {
            check_out_of_tree(p, &format!("surfaces[{i}].declared-at[{j}]"), true, &mut findings);
        }
    }

    if findings.is_empty() { Ok(()) } else { Err(Error::Validation { results: findings }) }
}

/// Validate semantic invariants on a `MetadataDocument`.
///
/// # Errors
///
/// Returns `Error::Validation` with one finding per violated invariant.
pub fn validate_metadata(doc: &MetadataDocument) -> Result<(), Error> {
    let mut findings = Vec::new();

    if doc.version != 1 {
        findings.push(finding(
            "metadata-version-unsupported",
            "metadata.json version must be 1",
            format!("found version {}", doc.version),
        ));
    }

    for (i, m) in doc.top_level_modules.iter().enumerate() {
        check_out_of_tree(m, &format!("top-level-modules[{i}]"), false, &mut findings);
    }

    if findings.is_empty() { Ok(()) } else { Err(Error::Validation { results: findings }) }
}

/// Is `p` an absolute filesystem path (Unix root, Windows backslash-root,
/// or Windows drive-letter prefix)?
#[must_use]
pub fn is_absolute_path(p: &str) -> bool {
    p.starts_with('/')
        || p.starts_with('\\')
        || (p.len() >= 3
            && p.as_bytes()[0].is_ascii_alphanumeric()
            && p.as_bytes()[1] == b':'
            && (p.as_bytes()[2] == b'/' || p.as_bytes()[2] == b'\\'))
}

/// Strip a trailing `:<line>` (decimal) from `declared-at` entries.
/// Returns `path` unchanged when there is no suffix.
#[must_use]
pub fn strip_line_suffix(path: &str) -> &str {
    if let Some((head, tail)) = path.rsplit_once(':')
        && !tail.is_empty()
        && tail.bytes().all(|b| b.is_ascii_digit())
    {
        return head;
    }
    path
}

fn has_parent_segment(p: &str) -> bool {
    p.split(['/', '\\']).any(|seg| seg == "..")
}

fn check_out_of_tree(
    path: &str, field_path: &str, allow_line_suffix: bool,
    findings: &mut Vec<specify_error::ValidationSummary>,
) {
    let candidate = if allow_line_suffix { strip_line_suffix(path) } else { path };
    if is_absolute_path(candidate) || has_parent_segment(candidate) {
        findings.push(finding(
            RULE_TOUCHES_OUT_OF_TREE,
            "paths must be relative and stay under the source root",
            format!("{field_path}: {path}"),
        ));
    }
}

fn finding(rule_id: &str, rule: &str, detail: String) -> specify_error::ValidationSummary {
    specify_error::ValidationSummary {
        status: specify_error::ValidationStatus::Fail,
        rule_id: rule_id.to_string(),
        rule: rule.to_string(),
        detail: Some(detail),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::survey::dto::{Surface, SurfaceKind};

    fn valid_surface() -> Surface {
        Surface {
            id: "http-get-users".to_string(),
            kind: SurfaceKind::HttpRoute,
            identifier: "GET /users".to_string(),
            handler: "src/routes/users.ts:listUsers".to_string(),
            touches: vec!["src/routes/users.ts".to_string(), "src/users/repository.ts".to_string()],
            declared_at: vec!["src/server.ts:42".to_string()],
        }
    }

    fn valid_doc() -> SurfacesDocument {
        SurfacesDocument {
            version: 1,
            source_key: "legacy-monolith".to_string(),
            language: "typescript".to_string(),
            surfaces: vec![valid_surface()],
        }
    }

    fn valid_metadata() -> MetadataDocument {
        MetadataDocument {
            version: 1,
            source_key: "legacy-monolith".to_string(),
            language: "typescript".to_string(),
            loc: 42_000,
            module_count: 15,
            top_level_modules: vec!["auth".to_string(), "billing".to_string()],
        }
    }

    #[test]
    fn valid_surfaces_ok() {
        validate_surfaces(&valid_doc()).unwrap();
    }

    #[test]
    fn valid_metadata_ok() {
        validate_metadata(&valid_metadata()).unwrap();
    }

    #[test]
    fn is_absolute_unix() {
        assert!(is_absolute_path("/etc/passwd"));
        assert!(is_absolute_path("/src/foo.ts:42"));
    }

    #[test]
    fn is_absolute_windows() {
        assert!(is_absolute_path("C:\\Users\\foo"));
        assert!(is_absolute_path("D:/project/bar"));
    }

    #[test]
    fn relative_not_absolute() {
        assert!(!is_absolute_path("src/foo.ts"));
        assert!(!is_absolute_path("src/foo.ts:42"));
        assert!(!is_absolute_path(""));
    }

    #[test]
    fn touches_absolute_path_is_out_of_tree() {
        let mut doc = valid_doc();
        doc.surfaces[0].touches = vec!["/absolute/path.ts".to_string()];
        let err = validate_surfaces(&doc).unwrap_err();
        assert_has_rule(&err, RULE_TOUCHES_OUT_OF_TREE);
    }

    #[test]
    fn touches_parent_segment_is_out_of_tree() {
        let mut doc = valid_doc();
        doc.surfaces[0].touches = vec!["src/../escaped/path.ts".to_string()];
        let err = validate_surfaces(&doc).unwrap_err();
        let detail = first_detail(&err, RULE_TOUCHES_OUT_OF_TREE);
        assert!(detail.contains("surfaces[0].touches[0]"), "field path missing: {detail}");
    }

    #[test]
    fn declared_at_parent_segment_is_out_of_tree() {
        let mut doc = valid_doc();
        doc.surfaces[0].declared_at = vec!["../escaped/path.ts:42".to_string()];
        let err = validate_surfaces(&doc).unwrap_err();
        let detail = first_detail(&err, RULE_TOUCHES_OUT_OF_TREE);
        assert!(detail.contains("surfaces[0].declared-at[0]"), "field path missing: {detail}");
    }

    #[test]
    fn declared_at_windows_path_is_out_of_tree() {
        let mut doc = valid_doc();
        doc.surfaces[0].declared_at = vec!["C:\\Windows\\path.ts:1".to_string()];
        let err = validate_surfaces(&doc).unwrap_err();
        assert_has_rule(&err, RULE_TOUCHES_OUT_OF_TREE);
    }

    fn assert_has_rule(err: &Error, expected_rule_id: &str) {
        let Error::Validation { results } = err else {
            panic!("expected Error::Validation, got: {err}");
        };
        assert!(
            results.iter().any(|r| r.rule_id == expected_rule_id),
            "expected finding `{expected_rule_id}` in {results:?}"
        );
    }

    fn first_detail(err: &Error, expected_rule_id: &str) -> String {
        let Error::Validation { results } = err else {
            panic!("expected Error::Validation, got: {err}");
        };
        results
            .iter()
            .find(|r| r.rule_id == expected_rule_id)
            .and_then(|r| r.detail.clone())
            .unwrap_or_else(|| panic!("expected finding `{expected_rule_id}` in {results:?}"))
    }
}
