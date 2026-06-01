//! Evidence authority enums — the closed `AuthorityClass` and
//! `ClaimKind` sets shared by `schemas/evidence.schema.json`,
//! `plan.yaml`'s per-slice `authority-override` map, and the slice
//! model.
//!
//! Authority resolves at document level via the per-Evidence
//! `authority:` field, with one operator override surface — the
//! per-slice `authority-override` on `plan.yaml`, keyed by claim kind
//! (decision-log §"Authority: document-level plus one override (v1)").
//! A per-Evidence `authority-overrides` map is deferred to a future
//! RFC.

use serde::{Deserialize, Serialize};

/// Closed authority-class enum mirrored from
/// `schemas/evidence.schema.json#/$defs/authorityClass`.
///
/// workflow §Authority hierarchy fixes the default ordering as
/// `intent > documentation > behaviour`; Evidence authority override lifts authority
/// from per-Evidence to per-(Evidence, claim-kind) without widening
/// the class set. New classes still require a workflow contract update.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, strum::Display, clap::ValueEnum,
)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum AuthorityClass {
    /// Operator intent — highest precedence.
    Intent,
    /// Written documentation, design notes, or briefs.
    Documentation,
    /// Observed runtime behaviour (legacy code, runtime captures).
    Behaviour,
}

/// Closed claim-kind enum mirrored from
/// `schemas/evidence.schema.json#/$defs/claimKind`.
///
/// Kept byte-identical with the schema enum so that the per-slice
/// authority override map in `plan.yaml.slices[]` validates against the
/// same closed set. `example` (runtime capture claim) is the runtime
/// capture kind emitted by the `captures` source adapter.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    strum::Display,
    strum::EnumString,
    clap::ValueEnum,
)]
#[serde(rename_all = "kebab-case")]
#[strum(
    serialize_all = "kebab-case",
    parse_err_ty = String,
    parse_err_fn = claim_kind_parse_error
)]
#[clap(rename_all = "kebab-case")]
pub enum ClaimKind {
    /// `kind: intent` — operator-stated intent (e.g. `change.md`).
    Intent,
    /// `kind: requirement` — behavioural requirement.
    Requirement,
    /// `kind: criterion` — acceptance criterion.
    Criterion,
    /// `kind: decision` — captured design decision.
    Decision,
    /// `kind: section` — documentation section anchor.
    Section,
    /// `kind: diagram` — diagram or architectural illustration.
    Diagram,
    /// `kind: contract` — interface contract excerpt.
    Contract,
    /// `kind: example` — runtime capture (runtime capture claim, `captures`).
    Example,
    /// `kind: excerpt` — code excerpt.
    Excerpt,
    /// `kind: type` — type definition.
    Type,
    /// `kind: call` — function or method call site.
    Call,
    /// `kind: region` — spatial region (`screenshots`).
    Region,
    /// `kind: container` — spatial container (`screenshots`).
    Container,
    /// `kind: leaf` — spatial leaf (`screenshots`).
    Leaf,
}

fn claim_kind_parse_error(other: &str) -> String {
    format!(
        "`{other}` is not a valid claim kind; expected one of intent, requirement, criterion, \
         decision, section, diagram, contract, example, excerpt, type, call, region, container, leaf"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authority_class_round_trips_kebab_case() {
        for (variant, wire) in [
            (AuthorityClass::Intent, "intent"),
            (AuthorityClass::Documentation, "documentation"),
            (AuthorityClass::Behaviour, "behaviour"),
        ] {
            let json = serde_json::to_string(&variant).expect("serialise");
            assert_eq!(json, format!("\"{wire}\""));
            let reparsed: AuthorityClass = serde_json::from_str(&json).expect("reparse");
            assert_eq!(variant, reparsed);
        }
    }

    #[test]
    fn claim_kind_round_trips_kebab_case() {
        let json = serde_json::to_string(&ClaimKind::Example).expect("serialise");
        assert_eq!(json, "\"example\"");
        let reparsed: ClaimKind = serde_json::from_str(&json).expect("reparse");
        assert_eq!(reparsed, ClaimKind::Example);
    }

    #[test]
    fn claim_kind_from_str_round_trips() {
        for variant in [
            ClaimKind::Intent,
            ClaimKind::Requirement,
            ClaimKind::Criterion,
            ClaimKind::Decision,
            ClaimKind::Section,
            ClaimKind::Diagram,
            ClaimKind::Contract,
            ClaimKind::Example,
            ClaimKind::Excerpt,
            ClaimKind::Type,
            ClaimKind::Call,
            ClaimKind::Region,
            ClaimKind::Container,
            ClaimKind::Leaf,
        ] {
            let wire = variant.to_string();
            let parsed: ClaimKind = wire.parse().expect("round-trip");
            assert_eq!(parsed, variant, "ClaimKind round-trip failed for {wire}");
        }
    }

    #[test]
    fn claim_kind_from_str_rejects_unknown() {
        let err = "bogus".parse::<ClaimKind>().expect_err("must reject unknown");
        assert!(err.contains("bogus"), "error must mention input, got: {err}");
    }
}
