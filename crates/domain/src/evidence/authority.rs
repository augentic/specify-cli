//! workflow §D2 — per-Evidence, per-claim-kind authority overrides.
//!
//! The default `intent > documentation > behaviour` ordering stays
//! per-Evidence via the existing `authority:` field; this module adds
//! the optional `authority-overrides` map that pins a per-kind
//! authority class for one Evidence document. Both keys (claim kinds)
//! and values (authority classes) reuse the existing closed enums
//! from `schemas/evidence.schema.json`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Closed authority-class enum mirrored from
/// `schemas/evidence.schema.json#/$defs/authorityClass`.
///
/// workflow §Authority hierarchy fixes the default ordering as
/// `intent > documentation > behaviour`; workflow §D2 lifts authority
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
/// Kept byte-identical with the schema enum so that
/// [`AuthorityOverrides`] and the per-slice authority override map
/// in `plan.yaml.slices[]` validate against the same closed set.
/// `example` (workflow §D1) is the runtime capture kind emitted by the
/// `captures` source adapter.
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
    /// `kind: example` — runtime capture (workflow §D1, `captures`).
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

/// Optional per-(Evidence, claim-kind) authority override map.
///
/// Synthesis consults this map first, then falls back to the
/// document-level `authority:` field, then to the workflow default
/// ordering. Empty maps and a missing field on the Evidence document
/// are equivalent — both leave the document-level `authority:`
/// untouched.
///
/// Wire shape (kebab-case keys, kebab-case values):
///
/// ```yaml
/// authority-overrides:
///   decision: documentation
///   criterion: behaviour
/// ```
///
/// Both keys and values are closed enums; new kinds or classes
/// require a workflow contract update.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AuthorityOverrides {
    /// Inner map. The outer struct is `#[serde(transparent)]` so the
    /// on-disk shape is the bare map under the `authority-overrides:`
    /// key on the parent Evidence document.
    pub by_kind: BTreeMap<ClaimKind, AuthorityClass>,
}

impl AuthorityOverrides {
    /// Lookup the override class for a given claim kind, if any.
    #[must_use]
    pub fn resolve(&self, kind: ClaimKind) -> Option<AuthorityClass> {
        self.by_kind.get(&kind).copied()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

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
    fn overrides_serialise_as_bare_map() {
        let overrides = AuthorityOverrides {
            by_kind: BTreeMap::from([
                (ClaimKind::Decision, AuthorityClass::Documentation),
                (ClaimKind::Criterion, AuthorityClass::Behaviour),
            ]),
        };
        let json = serde_json::to_string(&overrides).expect("serialise");
        assert_eq!(json, r#"{"criterion":"behaviour","decision":"documentation"}"#);
        let reparsed: AuthorityOverrides = serde_json::from_str(&json).expect("reparse");
        assert_eq!(reparsed, overrides);
    }

    #[test]
    fn overrides_resolve_returns_per_kind_class() {
        let overrides = AuthorityOverrides {
            by_kind: BTreeMap::from([(ClaimKind::Decision, AuthorityClass::Documentation)]),
        };
        assert_eq!(overrides.resolve(ClaimKind::Decision), Some(AuthorityClass::Documentation));
        assert_eq!(overrides.resolve(ClaimKind::Requirement), None);
    }

    #[test]
    fn claim_kind_from_str_round_trips_every_variant() {
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

    #[test]
    fn empty_overrides_round_trip() {
        let overrides = AuthorityOverrides::default();
        let json = serde_json::to_string(&overrides).expect("serialise");
        assert_eq!(json, "{}");
        assert!(overrides.by_kind.is_empty());
    }

    // ------------------------------------------------------------
    // workflow §D3 resolution-order pin.
    //
    // The CLI does not yet ship a synthesis resolver — `/spec:refine`
    // is still skill-driven and the in-Rust resolution helper lives
    // alongside `fusion.yaml` in the plugin repo (Change 3.2). This
    // test pins the four-step resolution walk at the data-structure
    // level so any future migration of the resolver into this crate
    // (and the parallel skill rewrite) inherits a frozen contract:
    //
    // 1. Per-slice `authority-override.<kind>` wins outright when it
    //    names a source key in the contributing group.
    // 2. Per-Evidence `authority-overrides.<kind>` wins when no
    //    per-slice override fired and at least one Evidence's
    //    override resolves to a strictly-greater authority class
    //    than the other contributors' effective class for the kind.
    // 3. Document-level `authority:` ordering decides when neither
    //    override surface fired and one contributor is strictly
    //    above the others.
    // 4. Same-effective-class tie → `Status: conflict` with no winner.
    //
    // Documented gap: when the resolver ships in this crate (RFC-27
    // Phase 3 follow-up), replace this micro-resolver with the
    // production code path and keep the four scenarios as black-box
    // assertions against it. The vocabulary (`PerSlice`, `PerEvidence`,
    // `Document`, `Conflict`) matches RFC-27's resolution-order step
    // names verbatim.
    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Resolved {
        PerSlice { winner: &'static str },
        PerEvidence { winner: &'static str, class: AuthorityClass },
        Document { winner: &'static str, class: AuthorityClass },
        Conflict { tied: Vec<&'static str> },
    }

    /// Toy contributor record used only by the resolution-order
    /// pin below. Production synthesis carries far more state (the
    /// claim payload, span, fixture digest, …) — the resolver
    /// itself only consults `(source, authority, overrides)` to
    /// break a tie.
    struct Contributor {
        source: &'static str,
        authority: AuthorityClass,
        overrides: AuthorityOverrides,
    }

    impl Contributor {
        fn effective(&self, kind: ClaimKind) -> AuthorityClass {
            self.overrides.resolve(kind).unwrap_or(self.authority)
        }
    }

    fn rank(class: AuthorityClass) -> u8 {
        // `intent > documentation > behaviour` per the workflow contract.
        match class {
            AuthorityClass::Intent => 2,
            AuthorityClass::Documentation => 1,
            AuthorityClass::Behaviour => 0,
        }
    }

    fn resolve(kind: ClaimKind, per_slice: Option<&str>, contributors: &[Contributor]) -> Resolved {
        // --- Step 1: per-slice override ---
        if let Some(target) = per_slice
            && contributors.iter().any(|c| c.source == target)
        {
            return Resolved::PerSlice {
                winner: contributors
                    .iter()
                    .find(|c| c.source == target)
                    .map(|c| c.source)
                    .expect("contributor present"),
            };
        }
        // --- Step 2: per-Evidence override widens the effective class ---
        let any_evidence_override =
            contributors.iter().any(|c| c.overrides.resolve(kind).is_some());
        let mut effective: Vec<(&Contributor, AuthorityClass)> =
            contributors.iter().map(|c| (c, c.effective(kind))).collect();
        effective.sort_by_key(|(c, _)| rank(c.authority));
        // Find the unique top class after applying per-Evidence overrides.
        let top_rank = effective.iter().map(|(_, cls)| rank(*cls)).max().unwrap_or(0);
        let top: Vec<&(&Contributor, AuthorityClass)> =
            effective.iter().filter(|(_, cls)| rank(*cls) == top_rank).collect();
        if top.len() == 1 {
            let (winner, class) = top[0];
            if any_evidence_override {
                return Resolved::PerEvidence {
                    winner: winner.source,
                    class: *class,
                };
            }
            return Resolved::Document {
                winner: winner.source,
                class: *class,
            };
        }
        // --- Step 4: tied at top class ---
        Resolved::Conflict {
            tied: top.iter().map(|(c, _)| c.source).collect(),
        }
    }

    #[test]
    fn resolution_order_step_1_per_slice_wins() {
        let contributors = vec![
            Contributor {
                source: "docs",
                authority: AuthorityClass::Documentation,
                overrides: AuthorityOverrides::default(),
            },
            Contributor {
                source: "runtime",
                authority: AuthorityClass::Behaviour,
                overrides: AuthorityOverrides::default(),
            },
        ];
        let resolved = resolve(ClaimKind::Criterion, Some("runtime"), &contributors);
        assert_eq!(resolved, Resolved::PerSlice { winner: "runtime" });
    }

    #[test]
    fn resolution_order_step_2_per_evidence_widens() {
        let contributors = vec![
            Contributor {
                source: "docs",
                authority: AuthorityClass::Documentation,
                overrides: AuthorityOverrides::default(),
            },
            Contributor {
                source: "runtime",
                authority: AuthorityClass::Behaviour,
                overrides: AuthorityOverrides {
                    by_kind: BTreeMap::from([(ClaimKind::Criterion, AuthorityClass::Intent)]),
                },
            },
        ];
        let resolved = resolve(ClaimKind::Criterion, None, &contributors);
        assert_eq!(
            resolved,
            Resolved::PerEvidence {
                winner: "runtime",
                class: AuthorityClass::Intent
            }
        );
    }

    #[test]
    fn resolution_order_step_3_document_authority_wins() {
        let contributors = vec![
            Contributor {
                source: "docs",
                authority: AuthorityClass::Documentation,
                overrides: AuthorityOverrides::default(),
            },
            Contributor {
                source: "runtime",
                authority: AuthorityClass::Behaviour,
                overrides: AuthorityOverrides::default(),
            },
        ];
        let resolved = resolve(ClaimKind::Criterion, None, &contributors);
        assert_eq!(
            resolved,
            Resolved::Document {
                winner: "docs",
                class: AuthorityClass::Documentation
            }
        );
    }

    #[test]
    fn resolution_order_step_4_tied_conflict() {
        let contributors = vec![
            Contributor {
                source: "docs-a",
                authority: AuthorityClass::Documentation,
                overrides: AuthorityOverrides::default(),
            },
            Contributor {
                source: "docs-b",
                authority: AuthorityClass::Documentation,
                overrides: AuthorityOverrides::default(),
            },
        ];
        let resolved = resolve(ClaimKind::Criterion, None, &contributors);
        let Resolved::Conflict { tied } = resolved else {
            panic!("expected tied conflict, got {resolved:?}");
        };
        let mut sorted = tied;
        sorted.sort_unstable();
        assert_eq!(sorted, vec!["docs-a", "docs-b"]);
    }

    #[test]
    fn resolution_order_per_slice_overrides_dominate_per_evidence() {
        // Operator intent: behaviour-class runtime captures
        // (`runtime`) should win this slice, overriding the
        // documentation evidence's authority-override that would
        // otherwise pick `docs` via step 2.
        let contributors = vec![
            Contributor {
                source: "docs",
                authority: AuthorityClass::Documentation,
                overrides: AuthorityOverrides {
                    by_kind: BTreeMap::from([(ClaimKind::Criterion, AuthorityClass::Intent)]),
                },
            },
            Contributor {
                source: "runtime",
                authority: AuthorityClass::Behaviour,
                overrides: AuthorityOverrides::default(),
            },
        ];
        let resolved = resolve(ClaimKind::Criterion, Some("runtime"), &contributors);
        assert_eq!(resolved, Resolved::PerSlice { winner: "runtime" });
    }
}
