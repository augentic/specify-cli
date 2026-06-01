//! Authority / status / winner derivation kernel (RFC-29c
//! §"Authority resolution", §"Status derivation", §"Provenance
//! projection").
//!
//! Given a requirement's contributing claims, the per-source document
//! authority classes, the per-slice `authority-override` map, and the
//! agent's `agreement` verdict, [`resolve`] derives the requirement's
//! [`RequirementStatus`], the per-claim winner markers, and the
//! [`ProvenanceResolution`] label the provenance projection recomputes
//! on demand. The function is pure and total: no filesystem, no clock,
//! no panic.
//!
//! Authority is keyed by `(source, kind)`, so one requirement can mix
//! claim kinds. The resolution order walked per claim is:
//!
//! 1. per-slice `authority-override[kind]` → the named source wins
//!    outright for that kind;
//! 2. document-level `authority`;
//! 3. the default class ordering `intent > documentation > behaviour`;
//! 4. a tie at the strictly-greatest effective class → `conflict`.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use specify_model::evidence::{AuthorityClass, ClaimKind};
use specify_model::spec::provenance::RequirementStatus;

use crate::slice::provenance::ProvenanceResolution;

/// The agent's agreement verdict for one requirement.
///
/// `Option<Agreement>` at the call boundary: the verdict is omitted for
/// a requirement with zero or one contributing claim, where there is no
/// agreement to record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, strum::Display)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum Agreement {
    /// Contributing claims describe the same value.
    Agreed,
    /// Contributing claims disagree; the kernel selects a winner by
    /// resolved authority.
    Disagreed,
}

/// One contributing claim, identified by the stable `(source, id,
/// kind)` triple the claim contract pins (RFC-29c §"Claim contract").
///
/// Resolution consults `source` and `kind`; `id` carries the claim's
/// identity so callers can pass real claim references and correlate the
/// returned winner markers back to `model.yaml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimRef {
    /// Source key (matches a top-level `plan.yaml.sources.<key>`).
    pub source: String,
    /// Claim id within that source's Evidence document.
    pub id: String,
    /// Claim kind (mirrors `schemas/evidence.schema.json#/$defs/claimKind`).
    pub kind: ClaimKind,
}

/// The kernel's verdict for one requirement.
///
/// `winners` is aligned index-for-index with the `claims` slice passed
/// to [`resolve`]: `None` on every claim of an `agreed` / single /
/// unknown requirement (no winner/loser distinction), and `Some(true)`
/// on the winning claim(s) with `Some(false)` on the losers of a
/// `divergence`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution {
    /// Derived requirement status stamped onto `model.yaml`.
    pub status: RequirementStatus,
    /// Recomputed provenance label (not persisted; the provenance
    /// projection derives it from the same inputs).
    pub resolution: ProvenanceResolution,
    /// Per-claim winner markers, aligned with the input claim order.
    pub winners: Vec<Option<bool>>,
}

impl Resolution {
    /// A resolution with no winner/loser distinction — every claim
    /// carries a `None` marker.
    fn uniform(
        claim_count: usize, status: RequirementStatus, resolution: ProvenanceResolution,
    ) -> Self {
        Self {
            status,
            resolution,
            winners: vec![None; claim_count],
        }
    }
}

/// Derive a requirement's status, winner markers, and provenance label.
///
/// `authority` maps each source key to its document-level
/// [`AuthorityClass`]; `overrides` is the per-slice
/// `authority-override` map (claim kind → winning source key). A source
/// missing from `authority` is treated as the lowest class
/// ([`AuthorityClass::Behaviour`]) — Evidence always carries an
/// `authority`, so this only guards malformed input.
#[must_use]
pub fn resolve(
    claims: &[ClaimRef], authority: &BTreeMap<String, AuthorityClass>,
    overrides: &BTreeMap<ClaimKind, String>, agreement: Option<Agreement>,
) -> Resolution {
    match claims.len() {
        0 => Resolution::uniform(
            0,
            RequirementStatus::Unknown,
            ProvenanceResolution::UnknownNoEvidence,
        ),
        1 => Resolution::uniform(1, RequirementStatus::Agreed, ProvenanceResolution::SingleSource),
        n => match agreement {
            Some(Agreement::Disagreed) => resolve_disagreement(claims, authority, overrides),
            // A well-formed verdict is present for ≥2 claims; an
            // absent one is read as agreement (no winner selection).
            Some(Agreement::Agreed) | None => Resolution::uniform(
                n,
                RequirementStatus::Agreed,
                ProvenanceResolution::SingleValueAgreement,
            ),
        },
    }
}

/// Select the winner among `disagreed` claims by strictly-greatest
/// effective authority. A unique top source yields `divergence`; a tie
/// at the top level yields `conflict`.
fn resolve_disagreement(
    claims: &[ClaimRef], authority: &BTreeMap<String, AuthorityClass>,
    overrides: &BTreeMap<ClaimKind, String>,
) -> Resolution {
    let levels: Vec<Level> =
        claims.iter().map(|claim| effective_level(claim, authority, overrides)).collect();
    // `claims` is non-empty (caller guarantees ≥2), so a max exists.
    let top = levels.iter().copied().max().unwrap_or(Level::Class(0));
    let winning_sources: BTreeSet<&str> = claims
        .iter()
        .zip(&levels)
        .filter(|(_, level)| **level == top)
        .map(|(claim, _)| claim.source.as_str())
        .collect();

    if winning_sources.len() == 1 {
        let winners = levels.iter().map(|level| Some(*level == top)).collect();
        let resolution = if top == Level::Override {
            ProvenanceResolution::PerSliceOverride
        } else {
            ProvenanceResolution::AuthorityResolved
        };
        Resolution {
            status: RequirementStatus::Divergence,
            resolution,
            winners,
        }
    } else {
        Resolution::uniform(
            claims.len(),
            RequirementStatus::Conflict,
            ProvenanceResolution::TiedConflict,
        )
    }
}

/// Effective authority level of one claim, after the per-slice override
/// step. `Override` outranks every document class so a forced source
/// wins outright.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Level {
    /// Document-level authority class, carried as its rank so the
    /// ordering is fixed independent of the `AuthorityClass` enum's
    /// declaration order (`behaviour < documentation < intent`).
    Class(u8),
    /// The per-slice `authority-override` forced this source to win for
    /// its claim's kind.
    Override,
}

/// Resolve one claim's effective level per the RFC-29c order: per-slice
/// override first, then document-level authority class.
fn effective_level(
    claim: &ClaimRef, authority: &BTreeMap<String, AuthorityClass>,
    overrides: &BTreeMap<ClaimKind, String>,
) -> Level {
    if overrides.get(&claim.kind).map(String::as_str) == Some(claim.source.as_str()) {
        return Level::Override;
    }
    let class = authority.get(&claim.source).copied().unwrap_or(AuthorityClass::Behaviour);
    Level::Class(class_rank(class))
}

/// Default class ordering `intent > documentation > behaviour`.
const fn class_rank(class: AuthorityClass) -> u8 {
    match class {
        AuthorityClass::Behaviour => 0,
        AuthorityClass::Documentation => 1,
        AuthorityClass::Intent => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claim(source: &str, id: &str, kind: ClaimKind) -> ClaimRef {
        ClaimRef {
            source: source.to_string(),
            id: id.to_string(),
            kind,
        }
    }

    fn authority(pairs: &[(&str, AuthorityClass)]) -> BTreeMap<String, AuthorityClass> {
        pairs.iter().map(|(source, class)| ((*source).to_string(), *class)).collect()
    }

    fn overrides(pairs: &[(ClaimKind, &str)]) -> BTreeMap<ClaimKind, String> {
        pairs.iter().map(|(kind, source)| (*kind, (*source).to_string())).collect()
    }

    // -- Status derivation table (RFC-29c §"Status derivation") -------

    #[test]
    fn zero_claims_are_unknown_no_evidence() {
        let resolved = resolve(&[], &authority(&[]), &overrides(&[]), None);
        assert_eq!(resolved.status, RequirementStatus::Unknown);
        assert_eq!(resolved.resolution, ProvenanceResolution::UnknownNoEvidence);
        assert!(resolved.winners.is_empty());
    }

    #[test]
    fn one_claim_is_agreed_single_source() {
        let claims = [claim("docs", "reset.request", ClaimKind::Requirement)];
        let resolved = resolve(
            &claims,
            &authority(&[("docs", AuthorityClass::Documentation)]),
            &overrides(&[]),
            None,
        );
        assert_eq!(resolved.status, RequirementStatus::Agreed);
        assert_eq!(resolved.resolution, ProvenanceResolution::SingleSource);
        assert_eq!(resolved.winners, vec![None]);
    }

    #[test]
    fn multi_agreed_is_single_value_agreement() {
        let claims = [
            claim("docs", "reset.request", ClaimKind::Requirement),
            claim("legacy", "users.reset.request", ClaimKind::Example),
        ];
        let resolved = resolve(
            &claims,
            &authority(&[
                ("docs", AuthorityClass::Documentation),
                ("legacy", AuthorityClass::Behaviour),
            ]),
            &overrides(&[]),
            Some(Agreement::Agreed),
        );
        assert_eq!(resolved.status, RequirementStatus::Agreed);
        assert_eq!(resolved.resolution, ProvenanceResolution::SingleValueAgreement);
        assert_eq!(resolved.winners, vec![None, None]);
    }

    #[test]
    fn multi_disagreed_unique_top_is_divergence() {
        let claims = [
            claim("docs", "reset.expiry", ClaimKind::Criterion),
            claim("legacy", "reset.expiry", ClaimKind::Example),
        ];
        let resolved = resolve(
            &claims,
            &authority(&[
                ("docs", AuthorityClass::Documentation),
                ("legacy", AuthorityClass::Behaviour),
            ]),
            &overrides(&[]),
            Some(Agreement::Disagreed),
        );
        assert_eq!(resolved.status, RequirementStatus::Divergence);
        assert_eq!(resolved.resolution, ProvenanceResolution::AuthorityResolved);
        assert_eq!(resolved.winners, vec![Some(true), Some(false)]);
    }

    #[test]
    fn multi_disagreed_tied_top_is_conflict() {
        let claims = [
            claim("docs-a", "reset.expiry", ClaimKind::Criterion),
            claim("docs-b", "reset.expiry", ClaimKind::Criterion),
        ];
        let resolved = resolve(
            &claims,
            &authority(&[
                ("docs-a", AuthorityClass::Documentation),
                ("docs-b", AuthorityClass::Documentation),
            ]),
            &overrides(&[]),
            Some(Agreement::Disagreed),
        );
        assert_eq!(resolved.status, RequirementStatus::Conflict);
        assert_eq!(resolved.resolution, ProvenanceResolution::TiedConflict);
        assert_eq!(resolved.winners, vec![None, None]);
    }

    // -- Resolution order (RFC-29c §"Authority resolution") -----------

    #[test]
    fn resolution_order_step_1_per_slice_override_wins() {
        // `runtime` is the lowest class but the override forces it to
        // win the `example` kind outright.
        let claims = [
            claim("docs", "reset.expiry", ClaimKind::Criterion),
            claim("runtime", "reset.expiry", ClaimKind::Example),
        ];
        let resolved = resolve(
            &claims,
            &authority(&[
                ("docs", AuthorityClass::Documentation),
                ("runtime", AuthorityClass::Behaviour),
            ]),
            &overrides(&[(ClaimKind::Example, "runtime")]),
            Some(Agreement::Disagreed),
        );
        assert_eq!(resolved.status, RequirementStatus::Divergence);
        assert_eq!(resolved.resolution, ProvenanceResolution::PerSliceOverride);
        assert_eq!(resolved.winners, vec![Some(false), Some(true)]);
    }

    #[test]
    fn resolution_order_step_2_document_authority_wins() {
        let claims = [
            claim("docs", "reset.expiry", ClaimKind::Criterion),
            claim("runtime", "reset.expiry", ClaimKind::Example),
        ];
        let resolved = resolve(
            &claims,
            &authority(&[
                ("docs", AuthorityClass::Documentation),
                ("runtime", AuthorityClass::Behaviour),
            ]),
            &overrides(&[]),
            Some(Agreement::Disagreed),
        );
        assert_eq!(resolved.status, RequirementStatus::Divergence);
        assert_eq!(resolved.resolution, ProvenanceResolution::AuthorityResolved);
        assert_eq!(resolved.winners, vec![Some(true), Some(false)]);
    }

    #[test]
    fn resolution_order_step_3_default_ordering_breaks_tie() {
        // `intent > documentation` decides when no override fires.
        let claims = [
            claim("brief", "reset.expiry", ClaimKind::Intent),
            claim("docs", "reset.expiry", ClaimKind::Criterion),
        ];
        let resolved = resolve(
            &claims,
            &authority(&[
                ("brief", AuthorityClass::Intent),
                ("docs", AuthorityClass::Documentation),
            ]),
            &overrides(&[]),
            Some(Agreement::Disagreed),
        );
        assert_eq!(resolved.status, RequirementStatus::Divergence);
        assert_eq!(resolved.resolution, ProvenanceResolution::AuthorityResolved);
        assert_eq!(resolved.winners, vec![Some(true), Some(false)]);
    }

    #[test]
    fn resolution_order_step_4_tie_is_conflict() {
        let claims = [
            claim("docs-a", "reset.expiry", ClaimKind::Criterion),
            claim("docs-b", "reset.expiry", ClaimKind::Criterion),
        ];
        let resolved = resolve(
            &claims,
            &authority(&[
                ("docs-a", AuthorityClass::Documentation),
                ("docs-b", AuthorityClass::Documentation),
            ]),
            &overrides(&[]),
            Some(Agreement::Disagreed),
        );
        assert_eq!(resolved.status, RequirementStatus::Conflict);
        assert_eq!(resolved.resolution, ProvenanceResolution::TiedConflict);
    }

    // -- Mixed-kind requirements (RFC-29c §"Per-claim resolution") ----

    #[test]
    fn mixed_kinds_per_kind_authority_picks_winner() {
        // A `criterion` (documentation) outranks an `example`
        // (behaviour) by the default ordering, no override.
        let claims = [
            claim("docs", "reset.expiry", ClaimKind::Criterion),
            claim("legacy", "reset.expiry", ClaimKind::Example),
        ];
        let resolved = resolve(
            &claims,
            &authority(&[
                ("docs", AuthorityClass::Documentation),
                ("legacy", AuthorityClass::Behaviour),
            ]),
            &overrides(&[]),
            Some(Agreement::Disagreed),
        );
        assert_eq!(resolved.status, RequirementStatus::Divergence);
        assert_eq!(resolved.resolution, ProvenanceResolution::AuthorityResolved);
        assert_eq!(resolved.winners, vec![Some(true), Some(false)]);
    }

    #[test]
    fn mixed_kinds_override_flips_winner() {
        // The default ordering would pick `docs`, but an override on the
        // `example` kind forces `legacy` to win.
        let claims = [
            claim("docs", "reset.expiry", ClaimKind::Criterion),
            claim("legacy", "reset.expiry", ClaimKind::Example),
        ];
        let resolved = resolve(
            &claims,
            &authority(&[
                ("docs", AuthorityClass::Documentation),
                ("legacy", AuthorityClass::Behaviour),
            ]),
            &overrides(&[(ClaimKind::Example, "legacy")]),
            Some(Agreement::Disagreed),
        );
        assert_eq!(resolved.status, RequirementStatus::Divergence);
        assert_eq!(resolved.resolution, ProvenanceResolution::PerSliceOverride);
        assert_eq!(resolved.winners, vec![Some(false), Some(true)]);
    }

    #[test]
    fn override_for_absent_source_does_not_fire() {
        // The override names `docs` for `criterion`, but no `docs`
        // criterion claim exists, so it falls through to authority.
        let claims = [
            claim("brief", "reset.expiry", ClaimKind::Intent),
            claim("legacy", "reset.expiry", ClaimKind::Example),
        ];
        let resolved = resolve(
            &claims,
            &authority(&[("brief", AuthorityClass::Intent), ("legacy", AuthorityClass::Behaviour)]),
            &overrides(&[(ClaimKind::Criterion, "docs")]),
            Some(Agreement::Disagreed),
        );
        assert_eq!(resolved.status, RequirementStatus::Divergence);
        assert_eq!(resolved.resolution, ProvenanceResolution::AuthorityResolved);
        assert_eq!(resolved.winners, vec![Some(true), Some(false)]);
    }

    #[test]
    fn override_only_fires_for_matching_kind() {
        // The override is keyed on `criterion`; `docs`'s `excerpt` claim
        // is not promoted, so behaviour-class authority is unchanged.
        let claims = [
            claim("docs", "reset.expiry", ClaimKind::Excerpt),
            claim("brief", "reset.expiry", ClaimKind::Intent),
        ];
        let resolved = resolve(
            &claims,
            &authority(&[
                ("docs", AuthorityClass::Documentation),
                ("brief", AuthorityClass::Intent),
            ]),
            &overrides(&[(ClaimKind::Criterion, "docs")]),
            Some(Agreement::Disagreed),
        );
        assert_eq!(resolved.status, RequirementStatus::Divergence);
        assert_eq!(resolved.resolution, ProvenanceResolution::AuthorityResolved);
        // `brief` (intent) wins; the override never fired.
        assert_eq!(resolved.winners, vec![Some(false), Some(true)]);
    }

    #[test]
    fn agreement_round_trips_kebab_case() {
        for (variant, wire) in [(Agreement::Agreed, "agreed"), (Agreement::Disagreed, "disagreed")]
        {
            let json = serde_json::to_string(&variant).expect("serialise");
            assert_eq!(json, format!("\"{wire}\""));
            let reparsed: Agreement = serde_json::from_str(&json).expect("reparse");
            assert_eq!(variant, reparsed);
        }
    }
}
