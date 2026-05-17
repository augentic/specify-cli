//! Lifecycle state machine for slice progression
//! (`Defining → Defined → Building → Complete → Merged`).
//! [`LifecycleStatus::transition`] is the only sanctioned mutator.

use specify_error::Error;

/// Lifecycle states a slice passes through.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    serde::Serialize,
    serde::Deserialize,
    strum::Display,
    clap::ValueEnum,
)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
#[non_exhaustive]
pub enum LifecycleStatus {
    /// Slice is being defined (artifacts authored).
    Defining,
    /// Definition complete, awaiting build.
    Defined,
    /// Build phase in progress.
    Building,
    /// Build complete, awaiting merge.
    Complete,
    /// Specs merged into baseline and slice archived.
    Merged,
    /// Slice discarded without merging.
    Dropped,
}

impl LifecycleStatus {
    /// The creation edge (`START → Defining`). Called by `init`/`define`.
    #[must_use]
    pub const fn initial() -> Self {
        Self::Defining
    }

    /// Whether this status is terminal (no further transitions possible).
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Merged | Self::Dropped)
    }

    /// Whether `self → target` is a legal edge in the lifecycle state machine.
    #[must_use]
    pub const fn can_transition_to(self, target: Self) -> bool {
        use LifecycleStatus::{Building, Complete, Defined, Defining, Dropped, Merged};
        matches!(
            (self, target),
            (Defining, Defined | Complete)
                | (Defined, Defining | Building)
                | (Building, Complete)
                | (Complete, Merged)
                | (Defining | Defined | Building | Complete, Dropped)
        )
    }

    /// Attempt a transition from `self` to `target`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Diag`] with `code = "lifecycle"` when `target`
    /// is not reachable from `self` per [`Self::can_transition_to`].
    /// The detail carries the rejected edge verbatim — callers and
    /// tests grep on the `lifecycle` discriminant for routing.
    pub fn transition(self, target: Self) -> Result<Self, Error> {
        if self.can_transition_to(target) {
            Ok(target)
        } else {
            Err(Error::Diag {
                code: "lifecycle",
                detail: format!("expected valid transition from {self:?}, found {target:?}"),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use clap::ValueEnum;

    use super::*;

    fn allowed_edges() -> HashSet<(LifecycleStatus, LifecycleStatus)> {
        use LifecycleStatus::*;
        let mut set = HashSet::new();
        set.insert((Defining, Defined));
        set.insert((Defined, Defining));
        set.insert((Defined, Building));
        set.insert((Building, Complete));
        set.insert((Complete, Merged));
        set.insert((Defining, Complete));
        // `any non-terminal -> Dropped`
        set.insert((Defining, Dropped));
        set.insert((Defined, Dropped));
        set.insert((Building, Dropped));
        set.insert((Complete, Dropped));
        set
    }

    #[test]
    fn initial_is_defining() {
        assert_eq!(LifecycleStatus::initial(), LifecycleStatus::Defining);
    }

    #[test]
    fn terminal_states_are_terminal() {
        assert!(LifecycleStatus::Merged.is_terminal());
        assert!(LifecycleStatus::Dropped.is_terminal());
        assert!(!LifecycleStatus::Defining.is_terminal());
        assert!(!LifecycleStatus::Defined.is_terminal());
        assert!(!LifecycleStatus::Building.is_terminal());
        assert!(!LifecycleStatus::Complete.is_terminal());
    }

    #[test]
    fn transition_table_matches_oracle() {
        let allowed = allowed_edges();
        for &from in LifecycleStatus::value_variants() {
            for &to in LifecycleStatus::value_variants() {
                let expected = allowed.contains(&(from, to));
                let actual = from.can_transition_to(to);
                assert_eq!(
                    actual, expected,
                    "({from:?}) -> ({to:?}): expected allowed={expected}, got {actual}"
                );
            }
        }
    }
}
