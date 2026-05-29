//! `kind: content-digest-eq` evaluator per RFC-34 §F6 (C16).
//!
//! Asserts that the content digest (SHA-256) of one file equals an
//! expected digest. v1 supports one source discriminator —
//! `agent-teams-match-canonical` — which consumes the
//! [`crate::lint::AgentTeam`] facts the framework-profile indexer
//! already produced (see [`crate::lint::index::agent_teams::record`])
//! and asserts that every `**/agent-teams.md` symlink resolves to
//! content whose digest equals the canonical
//! `docs/reference/review-team-protocol.md` review-team-protocol
//! document. The interpreter emits one [`crate::rules::Diagnostic`]
//! per symlink whose resolved-target digest diverges from the
//! canonical digest, with the symlink path as the finding's location
//! and the `(resolved-target, expected, actual)` shape surfaced via
//! [`crate::rules::FindingEvidence::Structured`] for downstream
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

use std::path::PathBuf;

use super::{HintError, make_finding};
use crate::lint::WorkspaceModel;
use crate::rules::{
    DeterministicHint, Diagnostic, FindingEvidence, FindingLocation, HintKind, ResolvedRule,
};

const SOURCE_AGENT_TEAMS_MATCH_CANONICAL: &str = "agent-teams-match-canonical";

/// Canonical review-team-protocol document, kept in sync with the
/// imperative `check::agent_teams` predicate's `CANONICAL_REL`. The
/// expected content digest is the digest any `agent-teams.md` symlink
/// carries when it resolves to this path.
const CANONICAL_REL: &str = "docs/reference/review-team-protocol.md";

/// Placeholder surfaced when a symlink's resolved target is
/// unreadable or broken (no `target-sha256` recorded). Distinct from
/// any real 64-char hex digest so it can never collide with a match.
const ABSENT_DIGEST_TOKEN: &str = "(unavailable)";

pub(crate) fn evaluate(
    rule: &ResolvedRule, hint: &DeterministicHint, _candidates: &[PathBuf], model: &WorkspaceModel,
    next_id: &mut u64,
) -> Result<Vec<Diagnostic>, HintError> {
    let source = hint.value.trim();
    if source != SOURCE_AGENT_TEAMS_MATCH_CANONICAL {
        return Err(HintError::Unsupported {
            rule_id: rule.rule_id.clone(),
            kind: HintKind::ContentDigestEq,
            reason: "only `agent-teams-match-canonical` is supported in v1",
        });
    }

    // Expected digest: the content digest carried by any symlink that
    // resolves to the canonical review-team-protocol document. Absent
    // canonical anchor means the invariant is vacuous this scan.
    let Some(expected) = model
        .agent_teams
        .iter()
        .find(|team| team.resolved_target.as_deref() == Some(CANONICAL_REL))
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
                team.path, resolved, actual_token, CANONICAL_REL, expected,
            ),
            data: serde_json::json!({
                "agent-team": team.path,
                "resolved-target": team.resolved_target,
                "canonical": CANONICAL_REL,
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
