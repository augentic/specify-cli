//! In-memory representation of one `## Candidate inventory` block.
//!
//! Mirrors `schemas/discovery/candidate.schema.json` — the kebab-case
//! `id`, the non-empty `sources[]` keys that surfaced the candidate,
//! the one-line `summary`, the optional `tentative` flag set by
//! `/spec:plan`'s `propose` sub-step on low-confidence cross-source
//! merges, and (discovery alias contract) the optional `aliases[]` list. Operator
//! additions through `specrun plan amend --add-alias` survive
//! re-enumeration.

use serde::{Deserialize, Serialize};

/// One block under `## Candidate inventory` in `discovery.md`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Candidate {
    /// Stable kebab-case identifier. Re-enumeration replaces by `id`.
    pub id: String,
    /// Non-empty list of source keys that surfaced this candidate.
    /// Each entry matches a top-level `plan.yaml.sources.<key>`
    /// binding; the on-disk schema rejects empty lists.
    pub sources: Vec<String>,
    /// One-line human-readable summary.
    pub summary: String,
    /// Optional uncertainty flag — set when `/spec:plan`'s `propose`
    /// sub-step merged this candidate across sources with low
    /// confidence; the operator reconciles at Gate 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tentative: Option<bool>,
    /// Optional alias list (discovery alias contract). `slices[].sources[].candidate`
    /// resolves first against `id`, then against any entry in
    /// `aliases`. Empty list and missing field are equivalent on the
    /// wire.
    #[serde(default, skip_serializing_if = "CandidateAliases::is_empty")]
    pub aliases: CandidateAliases,
}

/// Optional kebab-case alias list on a [`Candidate`].
///
/// `#[serde(transparent)]` over `Vec<String>` so the on-disk shape is
/// the bare YAML list under `aliases:`. Alias collisions with other
/// candidates' `id` or `aliases[]` are caught by `specrun slice
/// validate` (`discovery-alias-collision`); this type carries only
/// the per-candidate slot.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CandidateAliases {
    /// Backing storage. Order is significant for byte-stable diffs;
    /// the schema enforces uniqueness via `uniqueItems: true`.
    pub names: Vec<String>,
}

impl CandidateAliases {
    /// `true` when the alias list is empty (used by serde's
    /// `skip_serializing_if` to keep absent fields off the wire).
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    /// `true` when `needle` matches any alias entry (case-sensitive
    /// per discovery alias contract).
    #[must_use]
    pub fn contains(&self, needle: &str) -> bool {
        self.names.iter().any(|alias| alias == needle)
    }
}

impl<S> FromIterator<S> for CandidateAliases
where
    S: Into<String>,
{
    fn from_iter<I: IntoIterator<Item = S>>(iter: I) -> Self {
        Self {
            names: iter.into_iter().map(Into::into).collect(),
        }
    }
}

impl Candidate {
    /// `true` when `token` equals this candidate's `id` or any entry
    /// in `aliases[]`.
    ///
    /// discovery alias contract — `slices[].sources[].candidate` resolves first
    /// against `id`, then against `aliases[]`; case-sensitive.
    #[must_use]
    pub fn resolves(&self, token: &str) -> bool {
        self.id == token || self.aliases.contains(token)
    }

    /// Append `alias` to this candidate's `aliases[]`. Refuses when
    /// the value equals the candidate's own `id` (a no-op edit with
    /// no operator value); idempotent when `alias` is already
    /// present.
    ///
    /// Idempotency on exact-duplicate is the operator-ergonomic
    /// choice — `specrun plan amend --add-alias` is the operator's
    /// front door, and silently re-asserting a known alias is the
    /// least surprising shape there. Refusal on `id` collision is a
    /// clean signal: the operator either typed the wrong candidate
    /// or means to remove a stale alias, and either resolution
    /// belongs at the keyboard, not in the writer.
    ///
    /// Cross-candidate collisions (alias of this candidate equals
    /// some other candidate's `id` or alias) are NOT caught here —
    /// the caller resolves that via
    /// [`super::Discovery::check_alias_collisions`] before
    /// persisting, so a single CLI invocation can refuse the whole
    /// edit when any cross-candidate constraint trips.
    ///
    /// # Errors
    ///
    /// Returns [`AliasCollision::EqualsOwnId`] when `alias` equals
    /// `self.id`.
    pub fn add_alias(&mut self, alias: String) -> Result<(), AliasCollision> {
        if alias == self.id {
            return Err(AliasCollision::EqualsOwnId {
                candidate: self.id.clone(),
                alias,
            });
        }
        if self.aliases.contains(&alias) {
            return Ok(());
        }
        self.aliases.names.push(alias);
        Ok(())
    }

    /// Remove `alias` from this candidate's `aliases[]`. Idempotent
    /// — silently returns when the alias is not present so
    /// `specrun plan amend --remove-alias` can be issued without a
    /// prior probe.
    pub fn remove_alias(&mut self, alias: &str) {
        self.aliases.names.retain(|existing| existing != alias);
    }
}

/// Outcome of [`Candidate::add_alias`] when the operator-supplied
/// value cannot be appended.
///
/// Only the local "alias equals my own id" case lives here; whole-
/// document collisions are surfaced through
/// [`super::DiscoveryAliasCollision`] so callers see the same wire
/// shape whether the conflict was self-shadow or cross-candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AliasCollision {
    /// The supplied alias equals the candidate's own `id`. No-op
    /// edit; the operator likely typed the wrong target.
    EqualsOwnId {
        /// Candidate whose `add_alias` refused.
        candidate: String,
        /// Alias value that collided with the candidate's id.
        alias: String,
    },
}

impl std::fmt::Display for AliasCollision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EqualsOwnId { candidate, alias } => write!(
                f,
                "alias `{alias}` equals candidate `{candidate}`'s own id; aliases must name a \
                 different surface form"
            ),
        }
    }
}

impl std::error::Error for AliasCollision {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_minimal_block() {
        let yaml = r"id: user-registration
sources: [legacy]
summary: Registration endpoint accepting email + password.
";
        let parsed: Candidate = serde_saphyr::from_str(yaml).expect("parse");
        assert_eq!(parsed.id, "user-registration");
        assert!(parsed.aliases.is_empty(), "missing aliases must default to empty");
        assert_eq!(parsed.tentative, None);

        let rendered = serde_saphyr::to_string(&parsed).expect("serialise");
        assert!(!rendered.contains("aliases:"), "empty aliases must elide, got:\n{rendered}");
        assert!(!rendered.contains("tentative:"), "missing tentative must elide");
    }

    #[test]
    fn round_trips_with_aliases() {
        let yaml = r"id: user-registration
sources: [legacy, runtime]
summary: Registration endpoint accepting email + password.
aliases:
  - account-registration
  - user-signup
";
        let parsed: Candidate = serde_saphyr::from_str(yaml).expect("parse");
        assert_eq!(parsed.aliases.names, vec!["account-registration", "user-signup"]);

        let rendered = serde_saphyr::to_string(&parsed).expect("serialise");
        let reparsed: Candidate = serde_saphyr::from_str(&rendered).expect("reparse");
        assert_eq!(parsed, reparsed);
    }

    #[test]
    fn resolves_id_then_aliases() {
        let candidate = Candidate {
            id: "user-registration".to_string(),
            sources: vec!["legacy".to_string()],
            summary: "Registration.".to_string(),
            tentative: None,
            aliases: CandidateAliases::from_iter(["account-registration", "user-signup"]),
        };
        assert!(candidate.resolves("user-registration"));
        assert!(candidate.resolves("account-registration"));
        assert!(candidate.resolves("user-signup"));
        assert!(
            !candidate.resolves("USER-REGISTRATION"),
            "case-sensitive per discovery alias contract"
        );
        assert!(!candidate.resolves("password-reset"));
    }

    #[test]
    fn add_alias_appends_new_value() {
        let mut candidate = sample();
        candidate.add_alias("account-registration".to_string()).expect("ok");
        assert_eq!(candidate.aliases.names, vec!["account-registration"]);
    }

    #[test]
    fn add_alias_idempotent_on_exact_duplicate() {
        let mut candidate = sample();
        candidate.aliases = CandidateAliases::from_iter(["account-registration"]);
        candidate.add_alias("account-registration".to_string()).expect("idempotent ok");
        assert_eq!(candidate.aliases.names, vec!["account-registration"]);
    }

    #[test]
    fn add_alias_refuses_self_shadow() {
        let mut candidate = sample();
        let err =
            candidate.add_alias("user-registration".to_string()).expect_err("self-shadow refused");
        match err {
            AliasCollision::EqualsOwnId { candidate, alias } => {
                assert_eq!(candidate, "user-registration");
                assert_eq!(alias, "user-registration");
            }
        }
    }

    #[test]
    fn remove_alias_idempotent_when_absent() {
        let mut candidate = sample();
        candidate.aliases = CandidateAliases::from_iter(["x", "y"]);
        candidate.remove_alias("z");
        assert_eq!(candidate.aliases.names, vec!["x", "y"]);
    }

    #[test]
    fn remove_alias_drops_named_entry() {
        let mut candidate = sample();
        candidate.aliases = CandidateAliases::from_iter(["x", "y", "z"]);
        candidate.remove_alias("y");
        assert_eq!(candidate.aliases.names, vec!["x", "z"]);
    }

    fn sample() -> Candidate {
        Candidate {
            id: "user-registration".to_string(),
            sources: vec!["legacy".to_string()],
            summary: "Registration.".to_string(),
            tentative: None,
            aliases: CandidateAliases::default(),
        }
    }

    #[test]
    fn rejects_unknown_fields() {
        let yaml = r"id: user-registration
sources: [legacy]
summary: Registration.
rogue: true
";
        let err = serde_saphyr::from_str::<Candidate>(yaml)
            .expect_err("deny_unknown_fields must catch rogue");
        assert!(err.to_string().contains("rogue"), "expected error to name rogue, got: {err}");
    }
}
