//! `kind: fenced-block` evaluator (RFC-31 Phase 2).
//!
//! Consumes [`crate::lint::FencedBlock`] facts from the indexer and
//! applies closed `value` source discriminators. v1 ships
//! `skill-envelope-json-in-body` for CORE-037 parity.

use std::path::PathBuf;
use std::sync::LazyLock;

use regex::Regex;
use specify_diagnostics::{Diagnostic, FindingEvidence, FindingLocation};

use super::{HintError, make_finding};
use crate::lint::WorkspaceModel;
use crate::rules::{HintKind, ResolvedRule, RuleHint};

const SOURCE_SKILL_ENVELOPE_JSON_IN_BODY: &str = "skill-envelope-json-in-body";

static ENVELOPE_VERSION_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""envelope[-_]version"\s*:"#).expect("envelope version regex"));
static ENVELOPE_OK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""ok"\s*:\s*(true|false)\b"#).expect("envelope ok regex"));
static ENVELOPE_DATA_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""data"\s*:"#).expect("envelope data regex"));
static ENVELOPE_ERROR_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""error"\s*:\s*\{"#).expect("envelope error regex"));

fn is_envelope_body(body: &str) -> bool {
    if ENVELOPE_VERSION_RE.is_match(body) {
        return true;
    }
    let has_ok = ENVELOPE_OK_RE.is_match(body);
    let has_data = ENVELOPE_DATA_RE.is_match(body);
    let has_error = ENVELOPE_ERROR_RE.is_match(body);
    has_ok && (has_data || has_error)
}

fn is_json_fence(lang: &str) -> bool {
    lang == "json" || lang == "jsonc" || lang.starts_with("json ") || lang.starts_with("jsonc ")
}

pub(crate) fn evaluate(
    rule: &ResolvedRule, hint: &RuleHint, candidates: &[PathBuf], model: &WorkspaceModel,
    next_id: &mut u64,
) -> Result<Vec<Diagnostic>, HintError> {
    let source = hint.value.trim();
    if source != SOURCE_SKILL_ENVELOPE_JSON_IN_BODY {
        return Err(HintError::Unsupported {
            rule_id: rule.rule_id.clone(),
            kind: HintKind::FencedBlock,
            reason: "unknown fenced-block source discriminator",
        });
    }

    let candidate_set = super::candidate_set(candidates);
    let mut findings = Vec::new();

    for block in &model.fenced_blocks {
        if !candidate_set.contains(&block.path) {
            continue;
        }
        if !is_json_fence(&block.lang) {
            continue;
        }
        if !is_envelope_body(&block.body) {
            continue;
        }
        findings.push(make_finding(
            rule,
            *next_id,
            format!(
                "Envelope JSON in skill body: {} — block at line {} (link to docs/reference/cli-output-shapes.md instead of embedding the envelope shape)",
                block.path, block.line_start
            ),
            Some(FindingLocation {
                path: block.path.clone(),
                line: Some(block.line_start),
                column: None,
                end_line: None,
                end_column: None,
            }),
            FindingEvidence::Structured {
                summary: "envelope-json-in-body".to_string(),
                data: serde_json::json!({ "line-start": block.line_start }),
                locations: None,
            },
        ));
        *next_id += 1;
    }

    Ok(findings)
}
