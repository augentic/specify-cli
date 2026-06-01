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

    // ------------------------------------------------------------
    // per-slice authority override resolution-order pin (v1).
    //
    // The CLI does not yet ship a synthesis resolver — `/spec:refine`
    // is still skill-driven. This test pins the three-step resolution
    // walk at the data-structure level so any future migration of the
    // resolver into this crate (and the parallel skill rewrite)
    // inherits a frozen contract (decision-log §"Authority:
    // document-level plus one override (v1)"):
    //
    // 1. Per-slice `authority-override.<kind>` wins outright when it
    //    names a source key in the contributing group.
    // 2. Document-level `authority:` ordering decides when no
    //    per-slice override fired and one contributor is strictly
    //    above the others.
    // 3. Same-class tie → `Status: conflict` with no winner.
    //
    // The deferred per-Evidence `authority-overrides` surface (a future
    // RFC) would slot between steps 1 and 2; the vocabulary
    // (`PerSlice`, `Document`, `Conflict`) matches the RFC-29c
    // resolution-order step names verbatim.
    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Resolved {
        PerSlice { winner: &'static str },
        Document { winner: &'static str, class: AuthorityClass },
        Conflict { tied: Vec<&'static str> },
    }

    /// Toy contributor record used only by the resolution-order pin
    /// below. Production synthesis carries far more state (the claim
    /// payload, span, fixture digest, …) — the resolver itself only
    /// consults `(source, authority)` to break a tie.
    struct Contributor {
        source: &'static str,
        authority: AuthorityClass,
    }

    fn rank(class: AuthorityClass) -> u8 {
        // `intent > documentation > behaviour` per the workflow contract.
        match class {
            AuthorityClass::Intent => 2,
            AuthorityClass::Documentation => 1,
            AuthorityClass::Behaviour => 0,
        }
    }

    fn resolve(per_slice: Option<&str>, contributors: &[Contributor]) -> Resolved {
        // --- Step 1: per-slice override ---
        if let Some(target) = per_slice
            && let Some(c) = contributors.iter().find(|c| c.source == target)
        {
            return Resolved::PerSlice { winner: c.source };
        }
        // --- Step 2: document-level authority ordering ---
        let top_rank = contributors.iter().map(|c| rank(c.authority)).max().unwrap_or(0);
        let top: Vec<&Contributor> =
            contributors.iter().filter(|c| rank(c.authority) == top_rank).collect();
        if let [winner] = top[..] {
            return Resolved::Document {
                winner: winner.source,
                class: winner.authority,
            };
        }
        // --- Step 3: tied at top class ---
        Resolved::Conflict {
            tied: top.iter().map(|c| c.source).collect(),
        }
    }

    #[test]
    fn resolution_order_step_1_per_slice_wins() {
        let contributors = vec![
            Contributor { source: "docs", authority: AuthorityClass::Documentation },
            Contributor { source: "runtime", authority: AuthorityClass::Behaviour },
        ];
        let resolved = resolve(Some("runtime"), &contributors);
        assert_eq!(resolved, Resolved::PerSlice { winner: "runtime" });
    }

    #[test]
    fn resolution_order_step_2_document_authority_wins() {
        let contributors = vec![
            Contributor { source: "docs", authority: AuthorityClass::Documentation },
            Contributor { source: "runtime", authority: AuthorityClass::Behaviour },
        ];
        let resolved = resolve(None, &contributors);
        assert_eq!(
            resolved,
            Resolved::Document { winner: "docs", class: AuthorityClass::Documentation }
        );
    }

    #[test]
    fn resolution_order_step_3_tied_conflict() {
        let contributors = vec![
            Contributor { source: "docs-a", authority: AuthorityClass::Documentation },
            Contributor { source: "docs-b", authority: AuthorityClass::Documentation },
        ];
        let resolved = resolve(None, &contributors);
        let Resolved::Conflict { tied } = resolved else {
            panic!("expected tied conflict, got {resolved:?}");
        };
        let mut sorted = tied;
        sorted.sort_unstable();
        assert_eq!(sorted, vec!["docs-a", "docs-b"]);
    }
}
