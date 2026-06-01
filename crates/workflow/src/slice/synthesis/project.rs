//! Projection kernel — `project(response) -> SliceModel` (RFC-29c M2b,
//! §"Persist pipeline", §"Agent and kernel responsibilities").
//!
//! The agent owns cross-modal reconciliation — which requirements exist
//! and how claims merge or split. Everything around that judgment that
//! can be made deterministic is a pure projection over the structure the
//! agent returned: id assignment, authority resolution, status / winner
//! derivation, the rendered source list, and the stamped header. The
//! kernel **normalizes, never rejects** — any kernel-owned field the
//! agent happened to set (`id`, `status`, claim `winner`, rendered
//! `sources`) and any header field it supplied are ignored and
//! re-derived / re-stamped (RFC-29c §"Synthesis response").
//!
//! Four conditions the kernel cannot project around are hard aborts,
//! mirroring the drift findings `specrun slice validate` re-checks
//! (RFC-29c §"Drift validation"): a claim that anchors no on-disk
//! Evidence (`slice-model-source-orphan`), a claim whose `kind`
//! disagrees with Evidence (`slice-model-claim-kind-mismatch`), a
//! `satisfies[]` `REQ` ref with no projected target
//! (`slice-model-cross-ref-orphan`), and an id outside its closed
//! three-digit grammar (`slice-model-id-grammar`).
//!
//! [`project`] is pure: it performs no I/O and reads no clock. The
//! caller (the `specrun slice synthesize` handler) reads Evidence and
//! the plan to build the [`ProjectionHeader`], the per-source
//! `authority` map, the per-slice `overrides` map, and the
//! `evidence_claims` anchor index, then hands them in. **Kernel
//! determinism** (RFC-29c §"Shape-brief scope (D8)"): given fixed
//! inputs the output is byte-identical, and target-independent by
//! construction — no `target` or shape-brief input reaches this
//! function.

use std::collections::{BTreeMap, BTreeSet};

use specify_error::{Error, Result};
use specify_model::evidence::{AuthorityClass, ClaimKind};

use crate::slice::model::{ModelClaim, SliceModel};
use crate::slice::synthesis::authority::{ClaimRef, resolve};

/// The header fields the kernel stamps onto the persisted `model.yaml`
/// (RFC-29c §"Slice model (D4)").
///
/// Built by the caller from the slice's name and its bound project;
/// `target` is **not** a header field — it is resolved on demand from
/// `project`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectionHeader {
    /// Stored schema version (`const: 1` today).
    pub version: u32,
    /// Slice name (kebab-case).
    pub slice: String,
    /// Bound project name, when one is bound.
    pub project: Option<String>,
}

/// Project the agent's synthesis-response `model` into a fully-derived
/// persisted [`SliceModel`] (RFC-29c §"Persist pipeline" steps 1–2).
///
/// `model` is the structure the agent returned; any kernel-owned or
/// header field it set is ignored and re-derived. `header` is the
/// kernel-stamped header. `authority` maps each source key to its
/// document-level [`AuthorityClass`], `overrides` is the per-slice
/// `authority-override` map (claim kind → winning source), and
/// `evidence_claims` is the `(source, id) → kind` anchor index the
/// caller distilled from the on-disk Evidence documents.
///
/// The projection runs in this order, returning the first violation:
///
/// 1. **Claim anchoring** — every `(source, id, kind)` claim must anchor
///    an Evidence claim: an absent `(source, id)` aborts
///    `slice-model-source-orphan`; a recorded kind that differs aborts
///    `slice-model-claim-kind-mismatch`.
/// 2. **Id assignment** — `requirements[].id` is re-derived as
///    `REQ-NNN` in declaration order (zero-padded, no holes), ignoring
///    any agent-supplied id.
/// 3. **Resolve + derive** — per requirement, [`resolve`] derives
///    `status` and per-claim `winner` markers, and the rendered
///    `sources` list is ordered highest-effective-authority first
///    (ties broken by declaration order via a stable sort).
/// 4. **Header** — `version` / `slice` / `project` are stamped from
///    `header`.
/// 5. **Cross-refs + grammar** — every `tasks[].satisfies[]` `REQ` ref
///    must name a projected requirement (`slice-model-cross-ref-orphan`)
///    and every `tasks[].id` / `requirements[].id` must match its closed
///    three-digit grammar (`slice-model-id-grammar`).
///
/// # Errors
///
/// Returns [`Error::Validation`] (exit 2) carrying the first of the four
/// abort codes above that the response violates.
pub fn project(
    mut model: SliceModel, header: ProjectionHeader, authority: &BTreeMap<String, AuthorityClass>,
    overrides: &BTreeMap<ClaimKind, String>,
    evidence_claims: &BTreeMap<(String, String), ClaimKind>,
) -> Result<SliceModel> {
    // Step 1 — claim anchoring runs before projection: the kernel
    // cannot project an unanchored claim.
    check_claim_anchors(&model, evidence_claims)?;

    // Steps 2–3 — re-derive ids, status, winners, and rendered sources.
    for (index, requirement) in model.requirements.iter_mut().enumerate() {
        requirement.id = Some(format!("REQ-{:03}", index + 1));

        let claim_refs: Vec<ClaimRef> = requirement
            .claims
            .iter()
            .map(|claim| ClaimRef {
                source: claim.source.clone(),
                id: claim.id.clone(),
                kind: claim.kind,
            })
            .collect();
        let resolution = resolve(&claim_refs, authority, overrides, requirement.agreement);

        let sources = rendered_sources(&requirement.claims, authority, overrides);
        requirement.status = Some(resolution.status);
        for (claim, winner) in requirement.claims.iter_mut().zip(resolution.winners) {
            claim.winner = winner;
        }
        requirement.sources = sources;
    }

    // Step 4 — stamp the header, ignoring any agent-supplied values.
    model.version = Some(header.version);
    model.slice = Some(header.slice);
    model.project = header.project;

    // Step 5 — cross-ref then grammar over the now-projected ids.
    check_cross_refs(&model)?;
    check_id_grammar(&model)?;

    Ok(model)
}

/// Reject any claim that does not anchor an on-disk Evidence claim
/// (`slice-model-source-orphan`) or whose `kind` disagrees with the kind
/// Evidence records for that `(source, id)` (`slice-model-claim-kind-mismatch`)
/// — RFC-29c §"Claim contract (D13)".
fn check_claim_anchors(
    model: &SliceModel, evidence_claims: &BTreeMap<(String, String), ClaimKind>,
) -> Result<()> {
    for requirement in &model.requirements {
        for claim in &requirement.claims {
            let key = (claim.source.clone(), claim.id.clone());
            match evidence_claims.get(&key) {
                None => {
                    return Err(Error::validation_failed(
                        "slice-model-source-orphan",
                        "every claim references an Evidence (source, id)",
                        format!(
                            "claim ({}, {}) anchors no on-disk Evidence claim",
                            claim.source, claim.id
                        ),
                    ));
                }
                Some(recorded) if *recorded != claim.kind => {
                    return Err(Error::validation_failed(
                        "slice-model-claim-kind-mismatch",
                        "a claim kind matches the kind recorded in Evidence",
                        format!(
                            "claim ({}, {}) is kind '{}' but Evidence records '{}'",
                            claim.source, claim.id, claim.kind, recorded
                        ),
                    ));
                }
                Some(_) => {}
            }
        }
    }
    Ok(())
}

/// Reject any `tasks[].satisfies[]` `REQ` reference that does not name a
/// projected `requirements[].id` (`slice-model-cross-ref-orphan`,
/// RFC-29c §"Drift validation").
fn check_cross_refs(model: &SliceModel) -> Result<()> {
    let projected: BTreeSet<&str> =
        model.requirements.iter().filter_map(|req| req.id.as_deref()).collect();
    for task in &model.tasks {
        for satisfied in &task.satisfies {
            if !projected.contains(satisfied.as_str()) {
                return Err(Error::validation_failed(
                    "slice-model-cross-ref-orphan",
                    "every tasks[].satisfies REQ reference names a projected requirement",
                    format!("task '{}' satisfies unknown requirement '{satisfied}'", task.id),
                ));
            }
        }
    }
    Ok(())
}

/// Reject any `REQ` or `TASK` id outside its closed three-digit grammar
/// (`slice-model-id-grammar`, RFC-29c §"ID grammar"). `REQ` ids are
/// kernel-assigned and always pass; `TASK` ids are agent-authored, so
/// the gate is load-bearing for them.
fn check_id_grammar(model: &SliceModel) -> Result<()> {
    for requirement in &model.requirements {
        let id = requirement.id.as_deref().unwrap_or_default();
        if !matches_grammar(id, "REQ-") {
            return Err(id_grammar_error("requirement", id));
        }
    }
    for task in &model.tasks {
        if !matches_grammar(&task.id, "TASK-") {
            return Err(id_grammar_error("task", &task.id));
        }
    }
    Ok(())
}

/// `true` when `id` is `<prefix>NNN` with exactly three ASCII digits —
/// the closed `^REQ-[0-9]{3}$` / `^TASK-[0-9]{3}$` grammars.
fn matches_grammar(id: &str, prefix: &str) -> bool {
    id.strip_prefix(prefix)
        .is_some_and(|digits| digits.len() == 3 && digits.bytes().all(|byte| byte.is_ascii_digit()))
}

fn id_grammar_error(kind: &str, id: &str) -> Error {
    Error::validation_failed(
        "slice-model-id-grammar",
        "REQ and TASK ids match their closed three-digit grammar",
        format!("{kind} id '{id}' does not match its closed three-digit grammar"),
    )
}

/// Render the distinct source keys of `claims`, highest effective
/// authority first (RFC-29c §"Authority resolution"). Each source's
/// standing is the strongest effective level among its contributing
/// claims; a stable sort over the declaration-order list breaks ties by
/// first appearance.
fn rendered_sources(
    claims: &[ModelClaim], authority: &BTreeMap<String, AuthorityClass>,
    overrides: &BTreeMap<ClaimKind, String>,
) -> Vec<String> {
    let mut order: Vec<String> = Vec::new();
    for claim in claims {
        if !order.iter().any(|seen| seen == &claim.source) {
            order.push(claim.source.clone());
        }
    }
    order.sort_by(|left, right| {
        source_level(right, claims, authority, overrides)
            .cmp(&source_level(left, claims, authority, overrides))
    });
    order
}

/// The strongest effective [`Level`] one source reaches across its
/// contributing claims in a requirement.
fn source_level(
    source: &str, claims: &[ModelClaim], authority: &BTreeMap<String, AuthorityClass>,
    overrides: &BTreeMap<ClaimKind, String>,
) -> Level {
    claims
        .iter()
        .filter(|claim| claim.source == source)
        .map(|claim| effective_level(source, claim.kind, authority, overrides))
        .max()
        .unwrap_or(Level::Class(0))
}

/// Effective authority level of one `(source, kind)`, mirroring the
/// authority kernel's resolution order: a per-slice override outranks
/// every document class, otherwise the document-level class rank.
fn effective_level(
    source: &str, kind: ClaimKind, authority: &BTreeMap<String, AuthorityClass>,
    overrides: &BTreeMap<ClaimKind, String>,
) -> Level {
    if overrides.get(&kind).map(String::as_str) == Some(source) {
        return Level::Override;
    }
    let class = authority.get(source).copied().unwrap_or(AuthorityClass::Behaviour);
    Level::Class(class_rank(class))
}

/// Effective authority level for source ordering. The derived `Ord`
/// places every [`Level::Class`] below [`Level::Override`] (variant
/// declaration order), matching the authority kernel's `Override`
/// outranks every class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Level {
    /// Document-level class carried as its fixed rank
    /// (`behaviour < documentation < intent`).
    Class(u8),
    /// A per-slice `authority-override` forced this source to win.
    Override,
}

/// Default class ordering `intent > documentation > behaviour`,
/// independent of the `AuthorityClass` enum declaration order.
const fn class_rank(class: AuthorityClass) -> u8 {
    match class {
        AuthorityClass::Behaviour => 0,
        AuthorityClass::Documentation => 1,
        AuthorityClass::Intent => 2,
    }
}

#[cfg(test)]
mod tests {
    use specify_model::spec::provenance::RequirementStatus;

    use super::*;
    use crate::slice::model::{ModelRequirement, ModelTask, validate_model_doc};
    use crate::slice::synthesis::authority::Agreement;

    fn header() -> ProjectionHeader {
        ProjectionHeader {
            version: 1,
            slice: "identity-service".to_string(),
            project: Some("identity-service".to_string()),
        }
    }

    fn authority(pairs: &[(&str, AuthorityClass)]) -> BTreeMap<String, AuthorityClass> {
        pairs.iter().map(|(source, class)| ((*source).to_string(), *class)).collect()
    }

    fn evidence(pairs: &[(&str, &str, ClaimKind)]) -> BTreeMap<(String, String), ClaimKind> {
        pairs
            .iter()
            .map(|(source, id, kind)| (((*source).to_string(), (*id).to_string()), *kind))
            .collect()
    }

    fn claim(source: &str, id: &str, kind: ClaimKind) -> ModelClaim {
        ModelClaim {
            source: source.to_string(),
            id: id.to_string(),
            kind,
            winner: None,
        }
    }

    fn requirement(
        title: &str, statement: &str, agreement: Option<Agreement>, claims: Vec<ModelClaim>,
    ) -> ModelRequirement {
        ModelRequirement {
            id: None,
            title: title.to_string(),
            status: None,
            agreement,
            unit: Some("password-reset".to_string()),
            sources: Vec::new(),
            claims,
            statement: statement.to_string(),
            scenarios: Vec::new(),
            notes: None,
        }
    }

    fn task(id: &str, text: &str, satisfies: &[&str]) -> ModelTask {
        ModelTask {
            id: id.to_string(),
            text: text.to_string(),
            depends_on: Vec::new(),
            satisfies: satisfies.iter().map(|s| (*s).to_string()).collect(),
        }
    }

    /// The RFC-29c §"Slice model (D4)" envelope: REQ-001 is a
    /// multi-claim agreement; REQ-002 is a per-kind divergence where the
    /// documentation `criterion` beats the behaviour `example`.
    fn rfc_response() -> SliceModel {
        SliceModel {
            version: None,
            slice: None,
            project: None,
            requirements: vec![
                requirement(
                    "Request password reset",
                    "The system lets a registered user request a password reset link by email.",
                    Some(Agreement::Agreed),
                    vec![
                        claim("docs", "password-reset.request", ClaimKind::Requirement),
                        claim("legacy", "password-reset.request", ClaimKind::Example),
                    ],
                ),
                requirement(
                    "Reset link expiry",
                    "Reset links expire after 30 minutes.",
                    Some(Agreement::Disagreed),
                    vec![
                        claim("docs", "password-reset.expiry", ClaimKind::Criterion),
                        claim("legacy", "password-reset.expiry", ClaimKind::Example),
                    ],
                ),
            ],
            tasks: vec![task(
                "TASK-001",
                "Implement password reset request handling.",
                &["REQ-001"],
            )],
        }
    }

    fn rfc_authority() -> BTreeMap<String, AuthorityClass> {
        authority(&[("docs", AuthorityClass::Documentation), ("legacy", AuthorityClass::Behaviour)])
    }

    fn rfc_evidence() -> BTreeMap<(String, String), ClaimKind> {
        evidence(&[
            ("docs", "password-reset.request", ClaimKind::Requirement),
            ("legacy", "password-reset.request", ClaimKind::Example),
            ("docs", "password-reset.expiry", ClaimKind::Criterion),
            ("legacy", "password-reset.expiry", ClaimKind::Example),
        ])
    }

    #[test]
    fn projects_rfc_model_to_schema_valid_output() {
        let projected =
            project(rfc_response(), header(), &rfc_authority(), &BTreeMap::new(), &rfc_evidence())
                .expect("the RFC envelope projects");

        // Header stamped from `header`.
        assert_eq!(projected.version, Some(1));
        assert_eq!(projected.slice.as_deref(), Some("identity-service"));
        assert_eq!(projected.project.as_deref(), Some("identity-service"));

        // REQ-001 — multi-claim agreement, no winners, docs before legacy.
        let req1 = &projected.requirements[0];
        assert_eq!(req1.id.as_deref(), Some("REQ-001"));
        assert_eq!(req1.status, Some(RequirementStatus::Agreed));
        assert_eq!(req1.sources, vec!["docs", "legacy"]);
        assert!(req1.claims.iter().all(|c| c.winner.is_none()));

        // REQ-002 — documentation criterion beats behaviour example.
        let req2 = &projected.requirements[1];
        assert_eq!(req2.id.as_deref(), Some("REQ-002"));
        assert_eq!(req2.status, Some(RequirementStatus::Divergence));
        assert_eq!(req2.sources, vec!["docs", "legacy"]);
        assert_eq!(req2.claims[0].winner, Some(true));
        assert_eq!(req2.claims[1].winner, Some(false));

        // The projected model round-trips through the slice-model schema.
        let value = serde_json::to_value(&projected).expect("serialise projected model");
        validate_model_doc(&value).expect("projected model is schema-valid");
    }

    #[test]
    fn normalizes_agent_supplied_kernel_fields() {
        // The agent pre-assigns wrong kernel/header values; the kernel
        // ignores and re-derives every one of them (normalize, never
        // reject) — RFC-29c §"Synthesis response".
        let mut response = rfc_response();
        response.version = Some(99);
        response.slice = Some("bogus-slice".to_string());
        response.project = Some("bogus-project".to_string());
        response.requirements[0].id = Some("REQ-999".to_string());
        response.requirements[0].status = Some(RequirementStatus::Conflict);
        response.requirements[0].sources = vec!["wrong".to_string()];
        for claim in &mut response.requirements[0].claims {
            claim.winner = Some(true);
        }

        let projected =
            project(response, header(), &rfc_authority(), &BTreeMap::new(), &rfc_evidence())
                .expect("a normalizing projection succeeds");

        let req1 = &projected.requirements[0];
        assert_eq!(projected.version, Some(1));
        assert_eq!(projected.slice.as_deref(), Some("identity-service"));
        assert_eq!(projected.project.as_deref(), Some("identity-service"));
        assert_eq!(req1.id.as_deref(), Some("REQ-001"));
        assert_eq!(req1.status, Some(RequirementStatus::Agreed));
        assert_eq!(req1.sources, vec!["docs", "legacy"]);
        assert!(req1.claims.iter().all(|c| c.winner.is_none()));
    }

    #[test]
    fn kernel_is_deterministic() {
        // RFC-29c §"Shape-brief scope (D8)" kernel determinism: identical
        // inputs yield byte-identical output. Target-independence holds by
        // construction — `project` takes no `target` or shape-brief input.
        let first =
            project(rfc_response(), header(), &rfc_authority(), &BTreeMap::new(), &rfc_evidence())
                .expect("first projection");
        let second =
            project(rfc_response(), header(), &rfc_authority(), &BTreeMap::new(), &rfc_evidence())
                .expect("second projection");

        let first_json = serde_json::to_string(&first).expect("serialise first");
        let second_json = serde_json::to_string(&second).expect("serialise second");
        assert_eq!(first_json, second_json);
    }

    #[test]
    fn aborts_on_source_orphan() {
        let mut response = rfc_response();
        response.requirements[0].claims.push(claim("ghost", "no.such.claim", ClaimKind::Excerpt));

        let err = project(response, header(), &rfc_authority(), &BTreeMap::new(), &rfc_evidence())
            .expect_err("an unanchored claim aborts");
        match err {
            Error::Validation { code, .. } => assert_eq!(code, "slice-model-source-orphan"),
            other => panic!("expected slice-model-source-orphan, got {other:?}"),
        }
    }

    #[test]
    fn aborts_on_claim_kind_mismatch() {
        // Evidence records `password-reset.expiry` as a `criterion`; the
        // claim asserts `requirement`.
        let response = SliceModel {
            requirements: vec![requirement(
                "Reset link expiry",
                "Reset links expire after 30 minutes.",
                None,
                vec![claim("docs", "password-reset.expiry", ClaimKind::Requirement)],
            )],
            ..rfc_response()
        };

        let err = project(response, header(), &rfc_authority(), &BTreeMap::new(), &rfc_evidence())
            .expect_err("a kind mismatch aborts");
        match err {
            Error::Validation { code, .. } => assert_eq!(code, "slice-model-claim-kind-mismatch"),
            other => panic!("expected slice-model-claim-kind-mismatch, got {other:?}"),
        }
    }

    #[test]
    fn aborts_on_cross_ref_orphan() {
        let mut response = rfc_response();
        // Only REQ-001 / REQ-002 are projected; satisfy a missing REQ-003.
        response.tasks = vec![task("TASK-001", "Dangling satisfy.", &["REQ-003"])];

        let err = project(response, header(), &rfc_authority(), &BTreeMap::new(), &rfc_evidence())
            .expect_err("a dangling satisfies ref aborts");
        match err {
            Error::Validation { code, .. } => assert_eq!(code, "slice-model-cross-ref-orphan"),
            other => panic!("expected slice-model-cross-ref-orphan, got {other:?}"),
        }
    }

    #[test]
    fn aborts_on_id_grammar() {
        let mut response = rfc_response();
        // Agent-authored task id outside the closed grammar; its
        // (empty) satisfies list keeps the cross-ref check clean so the
        // grammar gate is what fires.
        response.tasks = vec![task("TASK-1", "Malformed task id.", &[])];

        let err = project(response, header(), &rfc_authority(), &BTreeMap::new(), &rfc_evidence())
            .expect_err("a malformed task id aborts");
        match err {
            Error::Validation { code, .. } => assert_eq!(code, "slice-model-id-grammar"),
            other => panic!("expected slice-model-id-grammar, got {other:?}"),
        }
    }
}
