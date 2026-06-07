//! Brief path-classification helpers.
//!
//! The brief size cap (CORE-013) is enforced by the config-driven
//! `cardinality` deterministic hints over the `Brief` fact family. This
//! module holds the pure parent/phase path classifiers, shared with the
//! brief-frontmatter parity reference.

static PARENT_BRIEF_NAMES: &[&str] =
    &["shape.md", "build.md", "merge.md", "survey.md", "extract.md"];

/// True for parent orchestrator briefs at `adapters/<axis>/<adapter>/briefs/{shape,build,merge,survey,extract}.md`.
#[must_use]
pub fn is_parent_brief(rel_path: &str) -> bool {
    let parts: Vec<&str> = rel_path.split('/').collect();
    if parts.len() != 5 {
        return false;
    }
    if parts[0] != "adapters" {
        return false;
    }
    if parts[1] != "targets" && parts[1] != "sources" {
        return false;
    }
    if parts[3] != "briefs" {
        return false;
    }
    PARENT_BRIEF_NAMES.contains(&parts[4])
}

/// True for phase sub-briefs under `adapters/<axis>/<adapter>/briefs/{build,extract}/**/*.md`.
#[must_use]
pub fn is_phase_sub_brief(rel_path: &str) -> bool {
    let parts: Vec<&str> = rel_path.split('/').collect();
    if parts.len() < 6 {
        return false;
    }
    if parts[0] != "adapters" {
        return false;
    }
    if parts[1] != "targets" && parts[1] != "sources" {
        return false;
    }
    if parts[3] != "briefs" {
        return false;
    }
    if parts[4] != "build" && parts[4] != "extract" {
        return false;
    }
    rel_path.ends_with(".md")
}

#[cfg(test)]
mod unit_tests {
    use super::*;

    #[test]
    fn parent_brief_path_classification() {
        assert!(is_parent_brief("adapters/targets/omnia/briefs/build.md"));
        assert!(!is_parent_brief("adapters/targets/omnia/briefs/build/crate.md"));
    }

    #[test]
    fn phase_sub_brief_path_classification() {
        assert!(is_phase_sub_brief("adapters/targets/omnia/briefs/build/crate.md"));
        assert!(!is_phase_sub_brief("adapters/targets/omnia/briefs/shape.md"));
    }
}
