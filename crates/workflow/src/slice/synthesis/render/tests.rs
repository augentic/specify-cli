use specify_model::spec::provenance::{RequirementTag, parse_spec_md};

use super::*;

/// REQ-001 (agreed, two sources) and REQ-002 (authority-resolved
/// divergence) — the worked example,
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
fn scenarios_render_as_headings_and_parse() {
    // Regression for augentic/specify#150: scenarios must render as
    // `#### Scenario:` H4 headings (not `- ` bullets) so the spec parser
    // and the `specs.requirements-have-scenarios` rule recognise them.
    let raw = "version: 1
slice: greeting
project: greeting
requirements:
  - id: REQ-001
    title: Fix the typo
    status: agreed
    unit: greeting
    sources: [intent]
    claims:
      - source: intent
        id: greeting
        kind: intent
    statement: The greeting renders the corrected spelling.
    scenarios:
      - 'Corrected spelling shown'
tasks: []
";
    let model = SliceModel::parse_yaml(raw).expect("model validates");
    let specs = render_spec_files(&model);
    assert_eq!(specs.len(), 1);

    assert!(
        specs[0].content.contains("#### Scenario: Corrected spelling shown"),
        "scenarios must render as #### Scenario: headings, got:\n{}",
        specs[0].content
    );
    assert!(
        !specs[0].content.contains("- Corrected spelling shown"),
        "scenarios must not render as bullets, got:\n{}",
        specs[0].content
    );

    // Provenance parser round-trips the block cleanly.
    let provenance = parse_spec_md(&specs[0].content);
    assert!(provenance.findings.is_empty(), "rendered scenario output parses cleanly");
    assert_eq!(provenance.requirements.len(), 1);

    // The baseline parser the `specs.requirements-have-scenarios` rule
    // consumes now recognises the rendered scenario.
    let baseline = specify_model::spec::parse_baseline(&specs[0].content);
    assert_eq!(baseline.requirements.len(), 1);
    assert_eq!(
        baseline.requirements[0].scenarios.len(),
        1,
        "the baseline parser recognises the rendered scenario"
    );
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
