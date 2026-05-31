//! In-memory representation of one `## Lead inventory` block.
//!
//! Mirrors `schemas/discovery/lead.schema.json` — one raw, unmerged
//! lead as surfaced by one source: the `source` that produced
//! it, the kebab-case `lead` (unique only within that
//! `source`), the content-bearing per-source `summary`, and
//! (discovery alias contract) the optional `aliases[]` list. Identity
//! is the `(source, lead)`
//! pair; cross-source unification is deferred to plan time, where
//! `/spec:plan`'s `propose` sub-step reads these leads but never edits
//! `discovery.md`. Operator additions through `specrun plan amend
//! --add-alias` survive re-survey.

use serde::{Deserialize, Serialize};

/// One raw, unmerged block under `## Lead inventory` in `discovery.md`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Lead {
    /// Stable kebab-case identifier, unique only within this lead's
    /// `source`. Re-survey of that source replaces the block by
    /// `(source, lead)`.
    pub lead: String,
    /// Source binding key that surfaced this lead. Matches a
    /// top-level `plan.yaml.sources.<key>` binding; a `survey`
    /// attributes every lead it produces to its own source key.
    pub source: String,
    /// Content-bearing per-source summary of the lead as this source
    /// surfaced it. SHOULD name the operation/surface and its salient
    /// constraint so a same-slug lead from another source can be
    /// matched or distinguished on content; MAY span more than one
    /// line. Plan-time headline material only — never slice-time
    /// `Evidence`.
    pub summary: String,
    /// Optional alias list (discovery alias contract).
    /// `slices[].sources[].lead` resolves first against `lead`,
    /// then against any entry in `aliases`, within this lead's
    /// `source`. Empty list and missing field are equivalent on
    /// the wire.
    #[serde(default, skip_serializing_if = "LeadAliases::is_empty")]
    pub aliases: LeadAliases,
}

/// Optional kebab-case alias list on a [`Lead`].
///
/// `#[serde(transparent)]` over `Vec<String>` so the on-disk shape is
/// the bare YAML list under `aliases:`. Alias collisions with other
/// leads' `id` or `aliases[]` are caught by `specrun slice
/// validate` (`discovery-alias-collision`); this type carries only
/// the per-lead slot.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LeadAliases {
    /// Backing storage. Order is significant for byte-stable diffs;
    /// the schema enforces uniqueness via `uniqueItems: true`.
    pub names: Vec<String>,
}

impl LeadAliases {
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

impl<S> FromIterator<S> for LeadAliases
where
    S: Into<String>,
{
    fn from_iter<I: IntoIterator<Item = S>>(iter: I) -> Self {
        Self {
            names: iter.into_iter().map(Into::into).collect(),
        }
    }
}

impl Lead {
    /// `true` when `token` equals this lead's `lead` or any entry
    /// in `aliases[]`.
    ///
    /// discovery alias contract — `slices[].sources[].lead`
    /// resolves first against `lead`, then against `aliases[]`;
    /// case-sensitive. Resolution is scoped to this lead's
    /// `source` by the caller.
    #[must_use]
    pub fn resolves(&self, token: &str) -> bool {
        self.lead == token || self.aliases.contains(token)
    }

    /// Append `alias` to this lead's `aliases[]`. Refuses when
    /// the value equals the lead's own `id` (a no-op edit with
    /// no operator value); idempotent when `alias` is already
    /// present.
    ///
    /// Idempotency on exact-duplicate is the operator-ergonomic
    /// choice — `specrun plan amend --add-alias` is the operator's
    /// front door, and silently re-asserting a known alias is the
    /// least surprising shape there. Refusal on `id` collision is a
    /// clean signal: the operator either typed the wrong lead
    /// or means to remove a stale alias, and either resolution
    /// belongs at the keyboard, not in the writer.
    ///
    /// Cross-lead collisions (alias of this lead equals
    /// some other lead's `id` or alias) are NOT caught here —
    /// the caller resolves that via
    /// [`super::Discovery::check_alias_collisions`] before
    /// persisting, so a single CLI invocation can refuse the whole
    /// edit when any cross-lead constraint trips.
    ///
    /// # Errors
    ///
    /// Returns [`AliasCollision::EqualsOwnId`] when `alias` equals
    /// `self.lead`.
    pub fn add_alias(&mut self, alias: String) -> Result<(), AliasCollision> {
        if alias == self.lead {
            return Err(AliasCollision::EqualsOwnId {
                lead: self.lead.clone(),
                alias,
            });
        }
        if self.aliases.contains(&alias) {
            return Ok(());
        }
        self.aliases.names.push(alias);
        Ok(())
    }

    /// Remove `alias` from this lead's `aliases[]`. Idempotent
    /// — silently returns when the alias is not present so
    /// `specrun plan amend --remove-alias` can be issued without a
    /// prior probe.
    pub fn remove_alias(&mut self, alias: &str) {
        self.aliases.names.retain(|existing| existing != alias);
    }
}

/// Outcome of [`Lead::add_alias`] when the operator-supplied
/// value cannot be appended.
///
/// Only the local "alias equals my own id" case lives here; whole-
/// document collisions are surfaced through
/// [`super::DiscoveryAliasCollision`] so callers see the same wire
/// shape whether the conflict was self-shadow or cross-lead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AliasCollision {
    /// The supplied alias equals the lead's own `lead`. No-op
    /// edit; the operator likely typed the wrong target.
    EqualsOwnId {
        /// Lead whose `add_alias` refused.
        lead: String,
        /// Alias value that collided with the lead's `lead`.
        alias: String,
    },
}

impl std::fmt::Display for AliasCollision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EqualsOwnId { lead, alias } => write!(
                f,
                "alias `{alias}` equals lead `{lead}`'s own lead; aliases must name a \
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
        let yaml = r"lead: user-registration
source: legacy
summary: Registration endpoint accepting email + password.
";
        let parsed: Lead = serde_saphyr::from_str(yaml).expect("parse");
        assert_eq!(parsed.lead, "user-registration");
        assert_eq!(parsed.source, "legacy");
        assert!(parsed.aliases.is_empty(), "missing aliases must default to empty");

        let rendered = serde_saphyr::to_string(&parsed).expect("serialise");
        assert!(!rendered.contains("aliases:"), "empty aliases must elide, got:\n{rendered}");
    }

    #[test]
    fn round_trips_with_aliases() {
        let yaml = r"lead: user-registration
source: legacy
summary: Registration endpoint accepting email + password.
aliases:
  - account-registration
  - user-signup
";
        let parsed: Lead = serde_saphyr::from_str(yaml).expect("parse");
        assert_eq!(parsed.aliases.names, vec!["account-registration", "user-signup"]);

        let rendered = serde_saphyr::to_string(&parsed).expect("serialise");
        let reparsed: Lead = serde_saphyr::from_str(&rendered).expect("reparse");
        assert_eq!(parsed, reparsed);
    }

    #[test]
    fn resolves_id_then_aliases() {
        let lead = Lead {
            lead: "user-registration".to_string(),
            source: "legacy".to_string(),
            summary: "Registration.".to_string(),
            aliases: LeadAliases::from_iter(["account-registration", "user-signup"]),
        };
        assert!(lead.resolves("user-registration"));
        assert!(lead.resolves("account-registration"));
        assert!(lead.resolves("user-signup"));
        assert!(!lead.resolves("USER-REGISTRATION"), "case-sensitive per discovery alias contract");
        assert!(!lead.resolves("password-reset"));
    }

    #[test]
    fn add_alias_appends_new_value() {
        let mut lead = sample();
        lead.add_alias("account-registration".to_string()).expect("ok");
        assert_eq!(lead.aliases.names, vec!["account-registration"]);
    }

    #[test]
    fn add_alias_idempotent_on_exact_duplicate() {
        let mut lead = sample();
        lead.aliases = LeadAliases::from_iter(["account-registration"]);
        lead.add_alias("account-registration".to_string()).expect("idempotent ok");
        assert_eq!(lead.aliases.names, vec!["account-registration"]);
    }

    #[test]
    fn add_alias_refuses_self_shadow() {
        let mut lead = sample();
        let err = lead.add_alias("user-registration".to_string()).expect_err("self-shadow refused");
        match err {
            AliasCollision::EqualsOwnId { lead, alias } => {
                assert_eq!(lead, "user-registration");
                assert_eq!(alias, "user-registration");
            }
        }
    }

    #[test]
    fn remove_alias_idempotent_when_absent() {
        let mut lead = sample();
        lead.aliases = LeadAliases::from_iter(["x", "y"]);
        lead.remove_alias("z");
        assert_eq!(lead.aliases.names, vec!["x", "y"]);
    }

    #[test]
    fn remove_alias_drops_named_entry() {
        let mut lead = sample();
        lead.aliases = LeadAliases::from_iter(["x", "y", "z"]);
        lead.remove_alias("y");
        assert_eq!(lead.aliases.names, vec!["x", "z"]);
    }

    fn sample() -> Lead {
        Lead {
            lead: "user-registration".to_string(),
            source: "legacy".to_string(),
            summary: "Registration.".to_string(),
            aliases: LeadAliases::default(),
        }
    }

    #[test]
    fn rejects_unknown_fields() {
        let yaml = r"lead: user-registration
source: legacy
summary: Registration.
rogue: true
";
        let err =
            serde_saphyr::from_str::<Lead>(yaml).expect_err("deny_unknown_fields must catch rogue");
        assert!(err.to_string().contains("rogue"), "expected error to name rogue, got: {err}");
    }
}
