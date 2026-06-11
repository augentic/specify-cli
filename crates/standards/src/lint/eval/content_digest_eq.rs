//! `kind: content-digest-eq` evaluator.
//!
//! Asserts that the content digest (SHA-256) of one surface equals an
//! expected digest. Two source discriminators:
//!
//! - `agent-teams-match-canonical` — consumes the
//!   [`crate::lint::AgentTeam`] facts the framework-profile indexer
//!   already produced (see [`crate::lint::index::agent_teams::record`])
//!   and asserts that every `**/agent-teams.md` symlink resolves to
//!   content whose digest equals the canonical review-team-protocol
//!   document named by `config.canonical-path`.
//! - `markdown-section` — pins a restated markdown section to its
//!   canonical home: the body under `config.section` in `config.path`
//!   must hash equal (modulo leading/trailing blank lines) to the body
//!   under `config.canonical-section` in `config.canonical-path`.
//!   Section line ranges come from the [`crate::lint::MarkdownSection`]
//!   facts; bodies are read from disk at evaluation time so no fact
//!   family grows a content payload.
//!
//! All paths and section titles are **policy supplied by the rule**,
//! never a `const` in this arm (per the standards-layer
//! policy-in-`specify` rule). The interpreter emits one [`specify_diagnostics::Diagnostic`]
//! per divergence, with the drifted surface as the finding's location
//! and the `(expected, actual)` digest shape surfaced via
//! [`specify_diagnostics::FindingEvidence::Structured`] for downstream
//! tooling.
//!
//! The expected canonical digest is sourced from the fact set itself:
//! the `target-sha256` carried by any symlink whose `resolved-target`
//! is the canonical relative path. In a healthy framework tree every
//! `agent-teams.md` symlink resolves to that one document, so the
//! expected digest is unambiguous and every symlink matches it — the
//! rule fires zero findings. A symlink that points at a different
//! document (a copy-paste, a stale overlay, a broken link with no
//! readable target) carries a divergent or absent digest and is
//! flagged. When no symlink resolves to the canonical document the
//! expected digest cannot be established and the interpreter emits no
//! findings; the imperative `check::agent_teams` predicate still owns
//! the missing-canonical / non-symlink branches (see the C16 parity
//! test docstring for why the imperative row is not retired).
//!
//! Unlike the other v1 fact-iterating evaluators, this kind does NOT
//! narrow by the `path-pattern` candidate set. The framework walker
//! records an `agent-teams.md` symlink as an [`crate::lint::AgentTeam`]
//! fact and does NOT emit a `file` fact for the symlink path, so the
//! file-derived candidate set the umbrella evaluator builds can never
//! select these facts. The full `agent_teams` fact family is the
//! candidate set; the `candidates` argument is accepted for dispatch
//! uniformity and intentionally unused.
//!
//! Future hint values may extend the closed source set; unknown
//! discriminators are rejected as [`super::HintError::Unsupported`]
//! so authoring drift surfaces at hint-evaluation time rather than
//! silently passing.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde::de::DeserializeOwned;
use specify_diagnostics::{Diagnostic, FindingEvidence, FindingLocation};
use specify_digest::sha256_hex;

use super::{HintError, make_finding};
use crate::lint::{MarkdownSection, WorkspaceModel};
use crate::rules::{HintKind, ResolvedRule, RuleHint};

const SOURCE_AGENT_TEAMS_MATCH_CANONICAL: &str = "agent-teams-match-canonical";
const SOURCE_MARKDOWN_SECTION: &str = "markdown-section";

/// Parsed `agent-teams-match-canonical` hint configuration. The
/// canonical-document path is policy supplied by the rule, not the
/// engine.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct ContentDigestEqConfig {
    canonical_path: String,
}

/// Parsed `markdown-section` hint configuration. Both `(path,
/// section)` pairs are policy supplied by the rule, not the engine.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct MarkdownSectionConfig {
    path: String,
    section: String,
    canonical_path: String,
    canonical_section: String,
}

fn parse_config<C: DeserializeOwned>(
    rule: &ResolvedRule, hint: &RuleHint, missing_reason: &'static str,
) -> Result<C, HintError> {
    let raw = hint.config.as_ref().ok_or_else(|| HintError::Unsupported {
        rule_id: rule.rule_id.clone(),
        kind: HintKind::ContentDigestEq,
        reason: missing_reason,
    })?;
    serde_json::from_value(raw.clone()).map_err(|_ignored| HintError::Unsupported {
        rule_id: rule.rule_id.clone(),
        kind: HintKind::ContentDigestEq,
        reason: "invalid content-digest-eq hint config JSON",
    })
}

/// Placeholder surfaced when a symlink's resolved target is
/// unreadable or broken (no `target-sha256` recorded). Distinct from
/// any real 64-char hex digest so it can never collide with a match.
const ABSENT_DIGEST_TOKEN: &str = "(unavailable)";

pub(crate) fn evaluate(
    rule: &ResolvedRule, hint: &RuleHint, _candidates: &[PathBuf], project_dir: &Path,
    model: &WorkspaceModel, next_id: &mut u64,
) -> Result<Vec<Diagnostic>, HintError> {
    match hint.value.trim() {
        SOURCE_AGENT_TEAMS_MATCH_CANONICAL => evaluate_agent_teams(rule, hint, model, next_id),
        SOURCE_MARKDOWN_SECTION => {
            evaluate_markdown_section(rule, hint, project_dir, model, next_id)
        }
        _ => Err(HintError::Unsupported {
            rule_id: rule.rule_id.clone(),
            kind: HintKind::ContentDigestEq,
            reason: "only `agent-teams-match-canonical` or `markdown-section` is supported in v1",
        }),
    }
}

fn evaluate_agent_teams(
    rule: &ResolvedRule, hint: &RuleHint, model: &WorkspaceModel, next_id: &mut u64,
) -> Result<Vec<Diagnostic>, HintError> {
    let cfg: ContentDigestEqConfig = parse_config(
        rule,
        hint,
        "`agent-teams-match-canonical` requires a `config: { canonical-path }`",
    )?;
    let canonical = cfg.canonical_path.as_str();

    // Expected digest: the content digest carried by any symlink that
    // resolves to the canonical review-team-protocol document. Absent
    // canonical anchor means the invariant is vacuous this scan.
    let Some(expected) = model
        .agent_teams
        .iter()
        .find(|team| team.resolved_target.as_deref() == Some(canonical))
        .and_then(|team| team.target_sha256.as_deref())
    else {
        return Ok(Vec::new());
    };

    let mut out: Vec<Diagnostic> = Vec::new();
    for team in &model.agent_teams {
        let actual = team.target_sha256.as_deref();
        if actual == Some(expected) {
            continue;
        }
        let actual_token = actual.unwrap_or(ABSENT_DIGEST_TOKEN);
        let resolved = team.resolved_target.as_deref().unwrap_or(&team.target_raw);
        let location = FindingLocation {
            path: team.path.clone(),
            line: Some(1),
            column: None,
            end_line: None,
            end_column: None,
        };
        let evidence = FindingEvidence::Structured {
            summary: format!(
                "agent-teams overlay '{}' resolves to '{}' with digest '{}' (canonical '{}' digest '{}')",
                team.path, resolved, actual_token, canonical, expected,
            ),
            data: serde_json::json!({
                "agent-team": team.path,
                "resolved-target": team.resolved_target,
                "canonical": canonical,
                "expected-digest": expected,
                "actual-digest": actual_token,
            }),
            locations: None,
        };
        let title = format!(
            "{}: agent-teams overlay '{}' content digest diverges from canonical",
            rule.title, team.path,
        );
        let finding = make_finding(rule, *next_id, title, Some(location), evidence);
        *next_id += 1;
        out.push(finding);
    }
    Ok(out)
}

/// `markdown-section` source: the pinned section's body must hash
/// equal to the canonical section's body. A missing section on either
/// side is itself a finding — the pin must fail loudly rather than
/// pass vacuously (nothing else guards the section's presence).
fn evaluate_markdown_section(
    rule: &ResolvedRule, hint: &RuleHint, project_dir: &Path, model: &WorkspaceModel,
    next_id: &mut u64,
) -> Result<Vec<Diagnostic>, HintError> {
    let cfg: MarkdownSectionConfig = parse_config(
        rule,
        hint,
        "`markdown-section` requires a `config: { path, section, canonical-path, canonical-section }`",
    )?;

    let pinned = find_section(model, &cfg.path, &cfg.section);
    let canonical = find_section(model, &cfg.canonical_path, &cfg.canonical_section);
    let (Some(pinned), Some(canonical)) = (pinned, canonical) else {
        let (path, section) = if pinned.is_none() {
            (&cfg.path, &cfg.section)
        } else {
            (&cfg.canonical_path, &cfg.canonical_section)
        };
        let finding = missing_section_finding(rule, &cfg, path, section, *next_id);
        *next_id += 1;
        return Ok(vec![finding]);
    };

    let pinned_body = section_body(project_dir, pinned);
    let canonical_body = section_body(project_dir, canonical);
    let actual = pinned_body.as_deref().map(|b| sha256_hex(b.as_bytes()));
    let expected = canonical_body.as_deref().map(|b| sha256_hex(b.as_bytes()));
    if actual.is_some() && actual == expected {
        return Ok(Vec::new());
    }

    let expected_token = expected.as_deref().unwrap_or(ABSENT_DIGEST_TOKEN);
    let actual_token = actual.as_deref().unwrap_or(ABSENT_DIGEST_TOKEN);
    let location = FindingLocation {
        path: pinned.path.clone(),
        line: Some(pinned.line_start),
        column: None,
        end_line: Some(pinned.line_end),
        end_column: None,
    };
    let evidence = FindingEvidence::Structured {
        summary: format!(
            "section '{}' in '{}' (digest '{}') diverges from canonical section '{}' in '{}' (digest '{}')",
            cfg.section,
            cfg.path,
            actual_token,
            cfg.canonical_section,
            cfg.canonical_path,
            expected_token,
        ),
        data: serde_json::json!({
            "path": cfg.path,
            "section": cfg.section,
            "canonical-path": cfg.canonical_path,
            "canonical-section": cfg.canonical_section,
            "expected-digest": expected_token,
            "actual-digest": actual_token,
        }),
        locations: None,
    };
    let title = format!(
        "{}: section '{}' in '{}' diverges from its canonical home",
        rule.title, cfg.section, cfg.path,
    );
    let finding = make_finding(rule, *next_id, title, Some(location), evidence);
    *next_id += 1;
    Ok(vec![finding])
}

/// First section fact in document order matching `(path, title)`.
fn find_section<'m>(
    model: &'m WorkspaceModel, path: &str, title: &str,
) -> Option<&'m MarkdownSection> {
    model
        .markdown_sections
        .iter()
        .filter(|section| section.path == path && section.title == title)
        .min_by_key(|section| section.line_start)
}

/// Section body read from disk: the lines after the heading through
/// the section's last line, with leading/trailing blank lines trimmed
/// and lines re-joined with `\n`. `None` when the file is unreadable.
fn section_body(project_dir: &Path, section: &MarkdownSection) -> Option<String> {
    let contents = std::fs::read_to_string(project_dir.join(&section.path)).ok()?;
    let lines: Vec<&str> = contents.lines().collect();
    let start = section.line_start as usize;
    let end = (section.line_end as usize).min(lines.len());
    let body = lines.get(start..end)?;
    let leading = body.iter().take_while(|line| line.trim().is_empty()).count();
    let trailing = body.iter().rev().take_while(|line| line.trim().is_empty()).count();
    let trimmed = &body[leading..body.len() - trailing.min(body.len() - leading)];
    Some(trimmed.join("\n"))
}

fn missing_section_finding(
    rule: &ResolvedRule, cfg: &MarkdownSectionConfig, path: &str, section: &str, id: u64,
) -> Diagnostic {
    let location = FindingLocation {
        path: path.to_string(),
        line: Some(1),
        column: None,
        end_line: None,
        end_column: None,
    };
    let evidence = FindingEvidence::Structured {
        summary: format!(
            "section '{section}' not found in '{path}' — the digest pin against '{}' § '{}' cannot be established",
            cfg.canonical_path, cfg.canonical_section,
        ),
        data: serde_json::json!({
            "path": cfg.path,
            "section": cfg.section,
            "canonical-path": cfg.canonical_path,
            "canonical-section": cfg.canonical_section,
            "missing-path": path,
            "missing-section": section,
        }),
        locations: None,
    };
    let title = format!("{}: pinned section '{section}' missing from '{path}'", rule.title);
    make_finding(rule, id, title, Some(location), evidence)
}
