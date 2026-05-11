//! Lifecycle state machine for slice progression.
//!
//! Legal edges: `Defining → Defined → Building → Complete → Merged`,
//! plus `Defining → Defined` reflux and `Defined → Defining` rewind,
//! with `Dropped` reachable from any non-terminal state. Terminal
//! states (`Merged`, `Dropped`) admit no outgoing edges;
//! [`LifecycleStatus::transition`] is the only sanctioned mutator.

use std::fmt;

use serde::{Deserialize, Serialize};
use specify_error::Error;

/// Lifecycle states a slice passes through.
#[derive(Debug, Copy, Clone, Deserialize, Serialize, PartialEq, Eq, Hash, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
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

impl fmt::Display for LifecycleStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Defining => "defining",
            Self::Defined => "defined",
            Self::Building => "building",
            Self::Complete => "complete",
            Self::Merged => "merged",
            Self::Dropped => "dropped",
        })
    }
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
    /// Returns [`Error::Lifecycle`] when `target` is not reachable from
    /// `self` per [`Self::can_transition_to`]. The error carries
    /// stringified `expected` and `found` fields so the JSON envelope
    /// surfaces the rejected edge verbatim — callers and tests grep on
    /// the `lifecycle` discriminant for routing.
    pub fn transition(self, target: Self) -> Result<Self, Error> {
        if self.can_transition_to(target) {
            Ok(target)
        } else {
            Err(Error::Lifecycle {
                expected: format!("valid transition from {self:?}"),
                found: format!("{target:?}"),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    const ALL_STATUSES: [LifecycleStatus; 6] = [
        LifecycleStatus::Defining,
        LifecycleStatus::Defined,
        LifecycleStatus::Building,
        LifecycleStatus::Complete,
        LifecycleStatus::Merged,
        LifecycleStatus::Dropped,
    ];

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
        for &from in &ALL_STATUSES {
            for &to in &ALL_STATUSES {
                let expected = allowed.contains(&(from, to));
                let actual = from.can_transition_to(to);
                assert_eq!(
                    actual, expected,
                    "({from:?}) -> ({to:?}): expected allowed={expected}, got {actual}"
                );
            }
        }
    }

    #[test]
    fn terminal_states_no_outgoing_edges() {
        for &from in &ALL_STATUSES {
            if !from.is_terminal() {
                continue;
            }
            for &to in &ALL_STATUSES {
                assert!(
                    !from.can_transition_to(to),
                    "terminal state {from:?} must not allow -> {to:?}"
                );
            }
        }
    }

    #[test]
    fn legal_edges_round_trip() {
        for (from, to) in allowed_edges() {
            let result = from
                .transition(to)
                .unwrap_or_else(|e| panic!("expected {from:?} -> {to:?} to succeed, got {e:?}"));
            assert_eq!(result, to);
        }
    }

    #[test]
    fn illegal_edges_return_lifecycle_error() {
        let allowed = allowed_edges();
        for &from in &ALL_STATUSES {
            for &to in &ALL_STATUSES {
                if allowed.contains(&(from, to)) {
                    continue;
                }
                let err = from
                    .transition(to)
                    .expect_err(&format!("{from:?} -> {to:?} should be rejected"));
                match err {
                    Error::Lifecycle { expected, found } => {
                        let from_dbg = format!("{from:?}");
                        let to_dbg = format!("{to:?}");
                        assert!(
                            expected.contains(&from_dbg),
                            "expected message {expected:?} should mention {from_dbg:?}"
                        );
                        assert!(
                            found.contains(&to_dbg),
                            "found message {found:?} should mention {to_dbg:?}"
                        );
                    }
                    other => panic!("expected Error::Lifecycle, got {other:?}"),
                }
            }
        }
    }

    #[test]
    fn lifecycle_status_display_matches_serde() {
        assert_eq!(LifecycleStatus::Defining.to_string(), "defining");
        assert_eq!(LifecycleStatus::Defined.to_string(), "defined");
        assert_eq!(LifecycleStatus::Building.to_string(), "building");
        assert_eq!(LifecycleStatus::Complete.to_string(), "complete");
        assert_eq!(LifecycleStatus::Merged.to_string(), "merged");
        assert_eq!(LifecycleStatus::Dropped.to_string(), "dropped");
    }
}
