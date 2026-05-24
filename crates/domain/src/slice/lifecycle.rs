//! Lifecycle state machine for slice progression
//! (`Refining → Refined → Built → Merged`, plus `* → Dropped` from any
//! non-terminal state). [`LifecycleStatus::transition`] is the only
//! sanctioned mutator.

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
pub enum LifecycleStatus {
    /// Slice directory created; `/spec:refine` extract + synthesis in flight.
    Refining,
    /// Canonical artifacts validated; ready for `/spec:build`.
    Refined,
    /// Tasks complete; ready for `/spec:merge`.
    Built,
    /// Specs merged into baseline and slice archived.
    Merged,
    /// Slice discarded without merging.
    Dropped,
}

impl LifecycleStatus {
    /// Attempt a transition. Legal edges: `Refining → Refined`,
    /// `Refined → Built`, `Built → Merged`, and
    /// `{Refining, Refined, Built} → Dropped`.
    ///
    /// # Errors
    /// `Error::Diag { code = "lifecycle", .. }` when not reachable;
    /// detail carries the rejected edge verbatim.
    pub fn transition(self, target: Self) -> Result<Self, Error> {
        use LifecycleStatus::{Built, Dropped, Merged, Refined, Refining};
        if matches!(
            (self, target),
            (Refining, Refined)
                | (Refined, Built)
                | (Built, Merged)
                | (Refining | Refined | Built, Dropped)
        ) {
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
    use super::*;

    #[test]
    fn transition_refining_to_refined_ok() {
        assert_eq!(
            LifecycleStatus::Refining.transition(LifecycleStatus::Refined).expect("legal"),
            LifecycleStatus::Refined,
        );
    }

    #[test]
    fn transition_rejects_skipping_states() {
        let Err(Error::Diag { code, detail }) =
            LifecycleStatus::Refining.transition(LifecycleStatus::Built)
        else {
            panic!("Refining -> Built skips Refined; must Err with lifecycle diag");
        };
        assert_eq!(code, "lifecycle");
        assert!(
            detail.contains("Refining") && detail.contains("Built"),
            "endpoints in: {detail:?}",
        );
    }
}
