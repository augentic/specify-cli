//! Deterministic baseline identity projection (RFC-36 §"Projection
//! contract").
//!
//! Projects a materialised project/slot directory into the
//! `surface[]` / `recent[]` pair carried by `.specify/topology.lock`
//! and the reconciliation envelope. The projection is purely
//! structural — unit slugs, requirement-block headings, and the
//! journal's `slice.archive.created` outcome summaries — never an LLM
//! summary, so the committed lock is verifiable by
//! regenerate-and-compare (D36-6).

use std::path::Path;

use specify_error::Error;
use specify_model::spec::provenance::parse_spec_md;

use super::topology::Surface;
use crate::config::Layout;
use crate::journal::{self, EventKind};

/// Maximum requirement titles projected per unit (RFC-36 §"Surface
/// bounds", `K`). A unit with more emits a `more:` count of the
/// elided tail rather than the titles themselves.
pub const SURFACE_TITLE_CAP: usize = 8;

/// Maximum `slice.archive.created` outcome summaries projected into
/// `recent[]` (RFC-36 §"Surface bounds", `M`). The tail suffices —
/// older merges are already reflected in `surface[]`.
pub const RECENT_TAIL: usize = 10;

/// Project `project_dir`'s baseline into the `(surface, recent)` pair.
///
/// `surface` enumerates every `.specify/specs/<unit>/spec.md`, sorted
/// by slug, each carrying up to [`SURFACE_TITLE_CAP`] requirement
/// titles in `REQ-NNN` id order plus a `more` count when capped.
/// `recent` is the last [`RECENT_TAIL`] `slice.archive.created`
/// outcome summaries from `.specify/journal.jsonl`, in append order.
/// A project with no baseline yields two empty vectors — greenfield
/// reconciliation degrades cleanly to `description` only.
///
/// # Errors
///
/// Surfaces I/O errors reading the specs tree or a `spec.md`, and any
/// error from reading the journal.
pub fn project_baseline(project_dir: &Path) -> Result<(Vec<Surface>, Vec<String>), Error> {
    let surface = project_surface(project_dir)?;
    let recent = project_recent(project_dir)?;
    Ok((surface, recent))
}

fn project_surface(project_dir: &Path) -> Result<Vec<Surface>, Error> {
    let specs_dir = Layout::new(project_dir).specify_dir().join("specs");
    if !specs_dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut units: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(&specs_dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let Some(unit) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        // Only project units that actually carry a baseline spec; an
        // empty unit directory is in-progress noise, not owned surface.
        if !entry.path().join("spec.md").is_file() {
            continue;
        }
        units.push(unit);
    }
    units.sort();

    let mut surfaces: Vec<Surface> = Vec::with_capacity(units.len());
    for unit in units {
        let text = std::fs::read_to_string(specs_dir.join(&unit).join("spec.md"))?;
        surfaces.push(project_unit(unit, &text));
    }
    Ok(surfaces)
}

/// Project one unit's `spec.md` into its bounded [`Surface`].
fn project_unit(unit: String, spec: &str) -> Surface {
    let parsed = parse_spec_md(spec);
    let mut ordered: Vec<(u64, String)> =
        parsed.requirements.into_iter().map(|req| (requirement_order(&req.id), req.name)).collect();
    // Stable sort by `REQ-NNN` id; requirements without an `ID:` line
    // sort to the tail while keeping document order among themselves.
    ordered.sort_by_key(|(order, _)| *order);
    let mut requirements: Vec<String> = ordered.into_iter().map(|(_, name)| name).collect();

    let total = requirements.len();
    let more = (total > SURFACE_TITLE_CAP).then(|| {
        requirements.truncate(SURFACE_TITLE_CAP);
        u64::try_from(total - SURFACE_TITLE_CAP).unwrap_or(u64::MAX)
    });
    Surface {
        unit,
        requirements,
        more,
    }
}

/// Sort key for a `REQ-NNN` id: the trailing integer, or [`u64::MAX`]
/// when the id is absent or unparseable so unlabelled requirements
/// stable-sort to the tail.
fn requirement_order(id: &str) -> u64 {
    id.rsplit('-').next().and_then(|n| n.parse().ok()).unwrap_or(u64::MAX)
}

fn project_recent(project_dir: &Path) -> Result<Vec<String>, Error> {
    let events = journal::read(Layout::new(project_dir))?;
    let mut summaries: Vec<String> = events
        .into_iter()
        .filter_map(|event| match event.kind {
            EventKind::SliceArchiveCreated { outcome_summary, .. } => Some(outcome_summary),
            _ => None,
        })
        .collect();
    let len = summaries.len();
    if len > RECENT_TAIL {
        summaries.drain(..len - RECENT_TAIL);
    }
    Ok(summaries)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn write_spec(project_dir: &Path, unit: &str, body: &str) {
        let dir = project_dir.join(".specify").join("specs").join(unit);
        fs::create_dir_all(&dir).expect("mkdir unit");
        fs::write(dir.join("spec.md"), body).expect("write spec.md");
    }

    fn requirement(id: &str, name: &str) -> String {
        format!("### Requirement: {name}\n\nID: {id}\n\nSome body.\n\n")
    }

    #[test]
    fn empty_baseline_projects_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (surface, recent) = project_baseline(dir.path()).expect("project");
        assert!(surface.is_empty());
        assert!(recent.is_empty());
    }

    #[test]
    fn units_sorted_by_slug_requirements_by_req_id() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_spec(
            dir.path(),
            "session",
            &format!(
                "{}{}",
                requirement("REQ-002", "Revoke session"),
                requirement("REQ-001", "Issue token")
            ),
        );
        write_spec(dir.path(), "password-reset", &requirement("REQ-001", "Request reset"));

        let (surface, _) = project_baseline(dir.path()).expect("project");
        assert_eq!(surface.len(), 2);
        assert_eq!(surface[0].unit, "password-reset");
        assert_eq!(surface[1].unit, "session");
        // `session` requirements are emitted in REQ id order, not the
        // (reversed) document order they were authored in.
        assert_eq!(surface[1].requirements, vec!["Issue token", "Revoke session"]);
        assert!(surface[1].more.is_none());
    }

    #[test]
    fn requirements_capped_with_more_count() {
        let dir = tempfile::tempdir().expect("tempdir");
        let body: String =
            (1..=12).map(|n| requirement(&format!("REQ-{n:03}"), &format!("Req {n}"))).collect();
        write_spec(dir.path(), "billing", &body);

        let (surface, _) = project_baseline(dir.path()).expect("project");
        assert_eq!(surface[0].requirements.len(), SURFACE_TITLE_CAP);
        assert_eq!(surface[0].more, Some(4));
        assert_eq!(surface[0].requirements[0], "Req 1");
    }

    #[test]
    fn recent_keeps_last_m_archive_summaries() {
        let dir = tempfile::tempdir().expect("tempdir");
        let layout = Layout::new(dir.path());
        let ts = journal::test_timestamp("2026-01-01T00:00:00Z");
        let mut events: Vec<journal::Event> = (1..=13)
            .map(|n| {
                journal::Event::new(
                    ts,
                    EventKind::SliceArchiveCreated {
                        slice_name: format!("s{n}"),
                        touched_specs: vec![format!("u{n}")],
                        outcome_summary: format!("u{n}: 1 modified"),
                        merge_sha: None,
                    },
                )
            })
            .collect();
        // An unrelated event kind must be ignored by the projection.
        events.push(journal::Event::new(
            ts,
            EventKind::PlanTransitionApproved {
                plan_name: "p".to_string(),
            },
        ));
        journal::append_batch(layout, &events).expect("append journal");

        let (_, recent) = project_baseline(dir.path()).expect("project");
        assert_eq!(recent.len(), RECENT_TAIL);
        assert_eq!(recent.first().map(String::as_str), Some("u4: 1 modified"));
        assert_eq!(recent.last().map(String::as_str), Some("u13: 1 modified"));
    }
}
