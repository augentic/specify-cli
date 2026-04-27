//! Spec-vs-code drift scaffolding (RFC-2, stub-level in Phase 1).
//!
//! This crate currently provides only the public types
//! (`Entry`, `Status`) and the `baseline_inventory` walker that
//! every later drift-detection routine will feed from. The actual "does the
//! generated code still match the baseline?" comparison lands with RFC-2.

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use specify_error::Error;
use specify_spec::{RequirementBlock, parse_baseline};

/// One row in a drift report: the status of a single requirement relative
/// to the source artefacts it claims to cover.
///
/// Serialised with `kebab-case` field names so the JSON/YAML shape is stable
/// across the Phase-1 CLI (`specify verify`, `specify drift` — to land in
/// RFC-2) without another serde churn later.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
#[must_use]
pub struct Entry {
    /// Stable requirement identifier (e.g. `REQ-001`).
    #[serde(rename = "requirement-id")]
    pub id: String,
    /// Human-readable requirement name.
    #[serde(rename = "requirement-name")]
    pub name: String,
    /// Drift classification for this requirement.
    pub status: Status,
    /// Optional detail about why the requirement drifted or is missing.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub detail: Option<String>,
}

/// Drift classification. `lowercase` matches the JSON shape used by
/// `specify-validate::ValidationResult` so CLI renderers can share
/// status-column formatting.
#[derive(Debug, Copy, Clone, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum Status {
    /// Requirement is fully covered by source artefacts.
    Covered,
    /// Requirement exists but source artefacts have diverged.
    Drifted,
    /// Requirement has no corresponding source artefacts.
    Missing,
    /// Source artefact exists without a corresponding requirement.
    Unspecified,
}

/// Walk `<specs_dir>/<name>/spec.md`, parse each baseline via
/// [`specify_spec::parse_baseline`], and return one
/// `(spec_name, requirement_blocks)` pair per spec.
///
/// `spec_name` is the directory name under `specs_dir` (not the file path).
/// The returned vec is sorted ascending by `spec_name` so callers get
/// deterministic output regardless of filesystem iteration order.
///
/// Behaviour details:
/// - **Missing `specs_dir`** returns `Ok(vec![])`. This is the
///   "fresh clone, no baseline yet" case and isn't an error.
/// - **Non-directory entries** at the top level of `specs_dir` (e.g. a
///   stray `README.md`) are silently skipped. Only subdirectories
///   containing a `spec.md` file contribute to the inventory.
/// - **Subdirectories without a `spec.md`** (empty or orphaned) are
///   silently skipped — they produce no entry.
/// - **Malformed spec bodies** — e.g. markdown files with no
///   `### Requirement:` headings — parse to a `ParsedSpec` with an empty
///   `requirements` vec. That's surfaced as `(name, vec![])`, not as an
///   error, because `specify-spec`'s parser is deliberately lenient
///   (coherence checking lives in `specify-merge` / `specify-validate`).
///
/// The return type is `Result` rather than an infallible `Vec<…>` so RFC-2
/// can tighten the spec parser without another public-API churn — today the
/// only failure mode is `Error::Io` bubbling up from the directory walk.
///
/// # Errors
///
/// Returns an error if the operation fails.
pub fn baseline_inventory(specs_dir: &Path) -> Result<Vec<(String, Vec<RequirementBlock>)>, Error> {
    if !specs_dir.exists() {
        return Ok(Vec::new());
    }

    let mut entries: Vec<(String, Vec<RequirementBlock>)> = Vec::new();
    for entry in fs::read_dir(specs_dir)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if !file_type.is_dir() {
            continue;
        }

        let spec_path = entry.path().join("spec.md");
        if !spec_path.is_file() {
            continue;
        }

        // Non-UTF-8 directory names under `specs/` aren't something the
        // spec format supports; skip them rather than erroring so a
        // stray weird directory doesn't brick the whole inventory.
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };

        let text = fs::read_to_string(&spec_path)?;
        let parsed = parse_baseline(&text);
        entries.push((name, parsed.requirements));
    }

    entries.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    const MINIMAL_SPEC: &str = "\
# Example

### Requirement: Login succeeds

ID: REQ-001

#### Scenario: Happy path

Given a valid user
When they sign in
Then they land on the dashboard
";

    fn write_spec(root: &Path, name: &str, body: &str) {
        let dir = root.join("specs").join(name);
        fs::create_dir_all(&dir).expect("create spec dir");
        fs::write(dir.join("spec.md"), body).expect("write spec.md");
    }

    #[test]
    fn missing_specs_dir_empty() {
        let tmp = tempdir().expect("tempdir");
        let specs = tmp.path().join("specs");
        let inventory = baseline_inventory(&specs).expect("inventory");
        assert!(inventory.is_empty());
    }

    #[test]
    fn one_spec_parsed() {
        let tmp = tempdir().expect("tempdir");
        write_spec(tmp.path(), "login", MINIMAL_SPEC);

        let inventory = baseline_inventory(&tmp.path().join("specs")).expect("inventory");
        assert_eq!(inventory.len(), 1);
        let (name, reqs) = &inventory[0];
        assert_eq!(name, "login");
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].id, "REQ-001");
        assert_eq!(reqs[0].name, "Login succeeds");
        assert_eq!(reqs[0].scenarios.len(), 1);
    }

    #[test]
    fn specs_sorted() {
        let tmp = tempdir().expect("tempdir");
        // Deliberately out of lexical order.
        write_spec(tmp.path(), "zed", MINIMAL_SPEC);
        write_spec(tmp.path(), "alpha", MINIMAL_SPEC);
        write_spec(tmp.path(), "mike", MINIMAL_SPEC);

        let inventory = baseline_inventory(&tmp.path().join("specs")).expect("inventory");
        let names: Vec<&str> = inventory.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, vec!["alpha", "mike", "zed"]);
    }

    #[test]
    fn ignores_stray_files() {
        let tmp = tempdir().expect("tempdir");
        write_spec(tmp.path(), "login", MINIMAL_SPEC);
        // Stray file at the top of specs/ — must not count as a spec.
        fs::write(tmp.path().join("specs").join("README.md"), "# Not a spec\n")
            .expect("write README");

        let inventory = baseline_inventory(&tmp.path().join("specs")).expect("inventory");
        assert_eq!(inventory.len(), 1);
        assert_eq!(inventory[0].0, "login");
    }

    #[test]
    fn skips_dir_without_spec_md() {
        let tmp = tempdir().expect("tempdir");
        // Orphan dir — no spec.md inside.
        fs::create_dir_all(tmp.path().join("specs").join("orphan")).expect("create orphan");
        write_spec(tmp.path(), "login", MINIMAL_SPEC);

        let inventory = baseline_inventory(&tmp.path().join("specs")).expect("inventory");
        assert_eq!(inventory.len(), 1);
        assert_eq!(inventory[0].0, "login");
    }

    #[test]
    fn malformed_spec_empty_requirements() {
        let tmp = tempdir().expect("tempdir");
        write_spec(
            tmp.path(),
            "broken",
            "# Just a markdown file\n\nNo requirement headings here.\n",
        );

        let inventory = baseline_inventory(&tmp.path().join("specs")).expect("inventory");
        assert_eq!(inventory.len(), 1);
        assert_eq!(inventory[0].0, "broken");
        assert!(inventory[0].1.is_empty());
    }
}
