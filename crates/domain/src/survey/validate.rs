//! Semantic validators for `SurfacesDocument` and `MetadataDocument`.
//!
//! JSON-schema validation catches structural errors (handled by callers
//! via `validate_against_schema`). The functions here enforce invariants
//! the schema cannot express: sorted lists, non-empty `declared-at`,
//! no absolute paths, and no duplicate surface ids.
//!
//! "No timestamps" and "no host-state leaks" are enforced by schema
//! closure (`additionalProperties: false`) plus the absolute-path check
//! below. No extra code is needed for those guardrails.

use std::collections::HashSet;

use specify_error::Error;

use super::dto::{MetadataDocument, SurfacesDocument};

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
    for s in &doc.surfaces {
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

        check_absolute_paths(&s.touches, &s.id, "touches", &mut findings);
        check_absolute_paths(&s.declared_at, &s.id, "declared-at", &mut findings);
    }

    check_absolute_path(&doc.source_key, "<root>", "source-key", &mut findings);

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

    check_absolute_path(&doc.source_key, "<root>", "source-key", &mut findings);

    for m in &doc.top_level_modules {
        check_absolute_path(m, "<root>", "top-level-modules", &mut findings);
    }

    if findings.is_empty() { Ok(()) } else { Err(Error::Validation { results: findings }) }
}

fn is_absolute_path(p: &str) -> bool {
    p.starts_with('/')
        || p.starts_with('\\')
        || (p.len() >= 3
            && p.as_bytes()[0].is_ascii_alphabetic()
            && p.as_bytes()[1] == b':'
            && (p.as_bytes()[2] == b'/' || p.as_bytes()[2] == b'\\'))
}

fn check_absolute_paths(
    paths: &[String], surface_id: &str, field: &str,
    findings: &mut Vec<specify_error::ValidationSummary>,
) {
    for p in paths {
        check_absolute_path(p, surface_id, field, findings);
    }
}

fn check_absolute_path(
    path: &str, surface_id: &str, field: &str, findings: &mut Vec<specify_error::ValidationSummary>,
) {
    if is_absolute_path(path) {
        findings.push(finding(
            "surfaces-absolute-path",
            "paths must be relative, not absolute",
            format!("absolute path in {field} of surface `{surface_id}`: {path}"),
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
}
