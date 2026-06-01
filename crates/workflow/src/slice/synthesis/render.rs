//! `spec.md` provenance-line rendering (RFC-29c M2b §"Rendering").
//!
//! The render step is the third synthesis phase: given a projected
//! [`SliceModel`], it emits one `specs/<unit>/spec.md` per owning unit,
//! injecting the kernel-owned `ID:` / `Sources:` / `Status:` provenance
//! lines (and the inline status tag) under each requirement heading.
//!
//! **Parser-symmetry decision.** The RFC's §"Rendering" sketch shows an
//! `## <title>` h2 heading, but that example is illustrative. This
//! renderer instead follows the requirement-block shape that
//! [`specify_model::spec::provenance::parse_spec_md`] consumes —
//! `### Requirement: <title>` plus an inline `[unknown]` / `[conflict]`
//! / `[divergence]` tag suffix when the status is not `agreed`, then the
//! three metadata lines, a blank line, and the body. Render and parse
//! stay symmetric so the C9 `slice-spec-provenance-stale` check (which
//! parses the on-disk `spec.md` with that same parser) and
//! `specrun slice validate` round-trip the output cleanly. The tag is
//! the status text itself (`divergence`, …), so the parser's
//! tag↔status coherence rule is satisfied by construction.
//!
//! The body is rendered purely from the model's behavioral prose
//! (`statement`, `scenarios`, `notes`) — the kernel fully owns the
//! provenance lines and the block structure is deterministic.

use std::collections::HashMap;
use std::fmt::Write as _;

use specify_model::spec::provenance::RequirementStatus;

use crate::slice::model::{ModelRequirement, SliceModel};

/// Unit key used to group requirements that carry no `unit` field.
const DEFAULT_UNIT: &str = "default";

const HEADING_PREFIX: &str = "### Requirement:";

/// One rendered `specs/<unit>/spec.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedSpec {
    /// Owning unit (the `specs/<unit>/spec.md` directory segment).
    pub unit: String,
    /// Full rendered Markdown content, provenance lines injected.
    pub content: String,
}

/// The canonical `(ID, Sources, Status)` provenance triplet a single
/// requirement renders, paired with its owning unit.
///
/// [`expected_provenance_lines`] returns one per requirement; the C9
/// staleness check (`slice-spec-provenance-stale`) parses the on-disk
/// `spec.md` with [`specify_model::spec::provenance::parse_spec_md`] and
/// compares each parsed requirement's `id` / `sources` / `status`
/// against this set. The field types mirror the parser's
/// [`specify_model::spec::provenance::Requirement`] so the comparison is
/// direct: `id` is a plain `String` (empty when unprojected), `status`
/// is the optional enum the parser yields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedRequirement {
    /// Owning unit.
    pub unit: String,
    /// Projected `REQ-NNN` id (empty when the model carried none).
    pub id: String,
    /// Rendered source list, highest authority first.
    pub sources: Vec<String>,
    /// Projected status, or `None` when the model carried none.
    pub status: Option<RequirementStatus>,
}

/// Render every `specs/<unit>/spec.md` from a projected [`SliceModel`].
///
/// Requirements are grouped by `unit` (declaration order of first
/// appearance), and within a unit render in declaration order. Each
/// requirement becomes one `### Requirement:` block carrying the
/// injected provenance lines followed by its behavioral body.
#[must_use]
pub fn render_spec_files(model: &SliceModel) -> Vec<RenderedSpec> {
    let mut order: Vec<String> = Vec::new();
    let mut blocks: HashMap<String, Vec<String>> = HashMap::new();
    for req in &model.requirements {
        let unit = unit_of(req);
        if !blocks.contains_key(&unit) {
            order.push(unit.clone());
        }
        blocks.entry(unit).or_default().push(render_block(req));
    }
    order
        .into_iter()
        .map(|unit| {
            let unit_blocks = blocks.remove(&unit).unwrap_or_default();
            let mut content = unit_blocks.join("\n\n");
            content.push('\n');
            RenderedSpec { unit, content }
        })
        .collect()
}

/// The canonical provenance triplet per requirement, in declaration
/// order, for the C9 staleness comparison against on-disk `spec.md`.
#[must_use]
pub fn expected_provenance_lines(model: &SliceModel) -> Vec<ExpectedRequirement> {
    model
        .requirements
        .iter()
        .map(|req| ExpectedRequirement {
            unit: unit_of(req),
            id: req.id.clone().unwrap_or_default(),
            sources: req.sources.clone(),
            status: req.status,
        })
        .collect()
}

fn unit_of(req: &ModelRequirement) -> String {
    req.unit.clone().unwrap_or_else(|| DEFAULT_UNIT.to_string())
}

fn render_block(req: &ModelRequirement) -> String {
    let mut out = String::new();
    out.push_str(HEADING_PREFIX);
    out.push(' ');
    out.push_str(&req.title);
    if let Some(status) = req.status
        && status != RequirementStatus::Agreed
    {
        let _ = write!(out, " [{status}]");
    }
    out.push('\n');
    let _ = writeln!(out, "ID: {}", req.id.as_deref().unwrap_or_default());
    let _ = writeln!(out, "Sources: {}", req.sources.join(", "));
    if let Some(status) = req.status {
        let _ = writeln!(out, "Status: {status}");
    }
    out.push('\n');
    out.push_str(&render_body(req));
    out
}

fn render_body(req: &ModelRequirement) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !req.statement.is_empty() {
        parts.push(req.statement.clone());
    }
    if !req.scenarios.is_empty() {
        parts.push(req.scenarios.iter().map(|s| format!("- {s}")).collect::<Vec<_>>().join("\n"));
    }
    if let Some(notes) = req.notes.as_deref().filter(|n| !n.is_empty()) {
        parts.push(notes.to_string());
    }
    parts.join("\n\n")
}

#[cfg(test)]
mod tests {
    use specify_model::spec::provenance::{RequirementTag, parse_spec_md};

    use super::*;

    /// REQ-001 (agreed, two sources) and REQ-002 (authority-resolved
    /// divergence) — the RFC-29c §"Slice model (D4)" worked example,
    /// already projected (kernel-owned `id` / `status` / `sources`
    /// present).
    fn worked_model() -> SliceModel {
        let raw = "version: 1
slice: identity-service
project: identity-service
requirements:
  - id: REQ-001
    title: Request password reset
    status: agreed
    unit: password-reset
    agreement: agreed
    sources: [docs, legacy]
    claims:
      - source: docs
        id: password-reset.request
        kind: requirement
      - source: legacy
        id: password-reset.request
        kind: example
    statement: The system lets a user request a reset link.
  - id: REQ-002
    title: Reset link expiry
    status: divergence
    unit: password-reset
    agreement: disagreed
    sources: [docs, legacy]
    claims:
      - source: docs
        id: password-reset.expiry
        kind: criterion
        winner: true
      - source: legacy
        id: password-reset.expiry
        kind: example
        winner: false
    statement: Reset links expire after 30 minutes.
tasks:
  - id: TASK-001
    text: Implement password reset request handling.
    satisfies: [REQ-001]
";
        SliceModel::parse_yaml(raw).expect("worked model must validate")
    }

    #[test]
    fn renders_agreed_block_exactly() {
        let model = worked_model();
        let req = &model.requirements[0];
        let block = render_block(req);
        assert_eq!(
            block,
            "### Requirement: Request password reset\n\
             ID: REQ-001\n\
             Sources: docs, legacy\n\
             Status: agreed\n\
             \n\
             The system lets a user request a reset link."
        );
    }

    #[test]
    fn agreed_block_round_trips_through_parser() {
        let model = worked_model();
        let specs = render_spec_files(&model);
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].unit, "password-reset");

        let parsed = parse_spec_md(&specs[0].content);
        assert!(parsed.findings.is_empty(), "rendered output parses cleanly");
        assert_eq!(parsed.requirements.len(), 2);

        let req = &parsed.requirements[0];
        assert_eq!(req.id, "REQ-001");
        assert_eq!(req.sources, vec!["docs".to_string(), "legacy".to_string()]);
        assert_eq!(req.status, Some(RequirementStatus::Agreed));
        assert_eq!(req.tag, None);
        assert_eq!(req.body, "The system lets a user request a reset link.");
    }

    #[test]
    fn divergence_emits_tag_and_round_trips() {
        let model = worked_model();
        let block = render_block(&model.requirements[1]);
        assert!(
            block.starts_with("### Requirement: Reset link expiry [divergence]\n"),
            "non-agreed status emits the matching heading tag: {block}"
        );

        let parsed = parse_spec_md(&block);
        let req = &parsed.requirements[0];
        assert_eq!(req.tag, Some(RequirementTag::Divergence));
        assert_eq!(req.status, Some(RequirementStatus::Divergence));
        assert_eq!(req.id, "REQ-002");
        // Tag↔status coherence: the parser's validator sees no mismatch.
        assert_eq!(req.tag.map(RequirementTag::expected_status), req.status);
    }

    #[test]
    fn expected_provenance_lines_match_model() {
        let model = worked_model();
        let expected = expected_provenance_lines(&model);
        assert_eq!(
            expected,
            vec![
                ExpectedRequirement {
                    unit: "password-reset".to_string(),
                    id: "REQ-001".to_string(),
                    sources: vec!["docs".to_string(), "legacy".to_string()],
                    status: Some(RequirementStatus::Agreed),
                },
                ExpectedRequirement {
                    unit: "password-reset".to_string(),
                    id: "REQ-002".to_string(),
                    sources: vec!["docs".to_string(), "legacy".to_string()],
                    status: Some(RequirementStatus::Divergence),
                },
            ]
        );
    }

    #[test]
    fn expected_lines_agree_with_parsed_render() {
        let model = worked_model();
        let specs = render_spec_files(&model);
        let parsed = parse_spec_md(&specs[0].content);
        let expected = expected_provenance_lines(&model);
        for (exp, req) in expected.iter().zip(&parsed.requirements) {
            assert_eq!(req.id, exp.id);
            assert_eq!(req.sources, exp.sources);
            assert_eq!(req.status, exp.status);
        }
    }
}
