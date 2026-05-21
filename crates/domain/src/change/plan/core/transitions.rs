//! Status transition rules and `Plan::transition` — the single writer
//! of [`Entry::status`] for `done`. The post-RFC-25 per-entry edge
//! table is two edges only:
//!
//! - `Pending → InProgress` (written by `plan next`, never here).
//! - `InProgress → Done` (written by `plan transition <name> done`,
//!   stamped by `/spec:merge`).
//!
//! Plan-level lifecycle transitions (`Pending → Reviewed`) live in the
//! sibling [`Plan::transition_lifecycle`].

use specify_error::Error;

use super::model::{Entry, Lifecycle, Plan, Status};

impl Status {
    /// Whether `self -> target` is a legal edge in the per-entry state
    /// machine.
    ///
    /// Post-RFC-25 there are exactly two legal edges:
    ///
    /// - `Pending → InProgress` (written only by `plan next`).
    /// - `InProgress → Done` (written only by `plan transition <name> done`).
    ///
    /// `Done` is terminal. Build failures and merge conflicts leave
    /// the active entry `InProgress` — there is no per-entry
    /// `failed`, `blocked`, or `skipped` state in v1.
    #[must_use]
    pub const fn can_transition_to(&self, target: &Self) -> bool {
        use Status::{Done, InProgress, Pending};
        matches!((self, target), (Pending, InProgress) | (InProgress, Done))
    }

    /// Return `target` if the edge is legal, otherwise an
    /// `Error::Diag` (code `plan-transition`) whose detail carries
    /// both endpoints by their `Debug` representation.
    ///
    /// # Errors
    ///
    /// Errors with `Error::Diag { code: "plan-transition", .. }` when
    /// `self -> target` is not in the legal edge table.
    pub fn transition(&self, target: Self) -> Result<Self, Error> {
        if self.can_transition_to(&target) {
            Ok(target)
        } else {
            Err(Error::Diag {
                code: "plan-transition",
                detail: format!("cannot transition from {self:?} to {target:?}"),
            })
        }
    }
}

impl Lifecycle {
    /// Whether `self -> target` is a legal plan-level lifecycle edge.
    ///
    /// Post-RFC-25 there is exactly one legal edge: `Pending → Reviewed`.
    /// `Reviewed` is terminal; the lifecycle does not move further during
    /// execution. "Currently executing" and "drained" are computed from
    /// per-entry [`Status`] at read time.
    #[must_use]
    pub const fn can_transition_to(&self, target: &Self) -> bool {
        matches!((self, target), (Self::Pending, Self::Reviewed))
    }
}

impl Plan {
    /// Transition the named entry to `target` per the per-entry edge
    /// table on [`Status::can_transition_to`].
    ///
    /// Post-RFC-25 the only target this writer accepts in practice is
    /// [`Status::Done`] — `Pending → InProgress` is reserved for
    /// `Plan::next` and not reachable through this path. The function
    /// still gates on the full edge table so an illegal `done` from
    /// `Pending` (or any other reachable state) surfaces as
    /// `plan-transition`.
    ///
    /// # Errors
    ///
    /// Errors when the entry is missing or when the edge is illegal.
    pub fn transition(&mut self, name: &str, target: Status) -> Result<(), Error> {
        let entry: &mut Entry =
            self.entries.iter_mut().find(|c| c.name == name).ok_or_else(|| Error::Diag {
                code: "plan-entry-not-found",
                detail: format!("no slice named '{name}' in plan"),
            })?;

        let new_status = entry.status.transition(target)?;
        entry.status = new_status;
        Ok(())
    }

    /// Transition the plan-level [`Plan::lifecycle`] to `target`.
    ///
    /// Post-RFC-25 the only legal edge is `Pending → Reviewed` — the
    /// operator stamp at Gate 1. `/spec:plan` MUST NOT call this (per
    /// RFC-25 §The plan gate); the verb is operator-only and skill
    /// bodies should stop at `pending` with the literal
    /// `specify plan transition <name> reviewed` hint.
    ///
    /// # Errors
    ///
    /// Errors with `Error::Diag { code: "plan-lifecycle-transition", .. }`
    /// when `self.lifecycle -> target` is not in the legal edge table.
    pub fn transition_lifecycle(&mut self, target: Lifecycle) -> Result<(), Error> {
        if self.lifecycle.can_transition_to(&target) {
            self.lifecycle = target;
            Ok(())
        } else {
            Err(Error::Diag {
                code: "plan-lifecycle-transition",
                detail: format!(
                    "cannot transition plan lifecycle from {:?} to {target:?}",
                    self.lifecycle
                ),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use clap::ValueEnum;

    use super::super::test_support::{change, plan_with_changes};
    use super::*;

    /// The legal per-entry edges post-RFC-25. Two edges only.
    fn allowed_edges() -> HashSet<(Status, Status)> {
        use Status::{Done, InProgress, Pending};
        let mut set = HashSet::new();
        set.insert((Pending, InProgress));
        set.insert((InProgress, Done));
        set
    }

    #[test]
    #[expect(clippy::iter_over_hash_type, reason = "test exhausts a HashSet of edges")]
    fn legal_edges_succeed() {
        for (from, to) in allowed_edges() {
            assert!(
                from.can_transition_to(&to),
                "{from:?} -> {to:?} should be allowed by can_transition_to"
            );
            let result = from
                .transition(to)
                .unwrap_or_else(|e| panic!("expected {from:?} -> {to:?} to succeed, got {e:?}"));
            assert_eq!(result, to);
        }
    }

    #[test]
    fn done_is_terminal() {
        for &t in Status::value_variants() {
            assert!(!Status::Done.can_transition_to(&t), "Done must not allow -> {t:?}");
        }
    }

    #[test]
    fn illegal_edges_rejected() {
        use Status::{Done, InProgress, Pending};
        let cases: &[(Status, Status)] = &[
            (Done, Pending),
            (Done, InProgress),
            (Pending, Done),
            (InProgress, Pending),
            (Pending, Pending),
            (InProgress, InProgress),
            (Done, Done),
        ];

        for &(from, to) in cases {
            assert!(
                !from.can_transition_to(&to),
                "{from:?} -> {to:?} must be rejected by can_transition_to"
            );
            let err = from.transition(to).expect_err(&format!("{from:?} -> {to:?} should be Err"));
            match err {
                Error::Diag {
                    code: "plan-transition",
                    detail,
                } => {
                    assert!(
                        detail.contains(&format!("{from:?}"))
                            && detail.contains(&format!("{to:?}")),
                        "expected both endpoints in detail, got {detail:?}"
                    );
                }
                other => panic!("expected Error::Diag(plan-transition), got {other:?}"),
            }
        }
    }

    #[test]
    fn table_matches_oracle() {
        let allowed = allowed_edges();
        for &from in Status::value_variants() {
            for &to in Status::value_variants() {
                let expected = allowed.contains(&(from, to));
                let actual = from.can_transition_to(&to);
                assert_eq!(
                    actual, expected,
                    "({from:?}) -> ({to:?}): expected allowed={expected}, got {actual}"
                );
            }
        }
    }

    #[test]
    fn error_carries_endpoints() {
        let err = Status::Done.transition(Status::Pending).expect_err("Done -> Pending must error");
        match err {
            Error::Diag {
                code: "plan-transition",
                detail,
            } => {
                assert!(detail.contains("Done"), "detail should mention Done: {detail}");
                assert!(detail.contains("Pending"), "detail should mention Pending: {detail}");
            }
            other => panic!("expected Error::Diag(plan-transition), got {other:?}"),
        }
    }

    #[test]
    fn transition_in_progress_to_done() {
        let mut plan =
            plan_with_changes(vec![change("a", Status::Pending), change("b", Status::InProgress)]);

        plan.transition("b", Status::Done).expect("in-progress -> done ok");
        let b = plan.entries.iter().find(|c| c.name == "b").unwrap();
        assert_eq!(b.status, Status::Done);
    }

    #[test]
    fn transition_rejects_illegal_edge() {
        let mut plan = plan_with_changes(vec![change("a", Status::Done)]);
        let err = plan
            .transition("a", Status::Pending)
            .expect_err("Done -> Pending must Err from state machine");
        match err {
            Error::Diag {
                code: "plan-transition",
                detail,
            } => {
                assert!(detail.contains("Done"), "detail should mention Done: {detail}");
                assert!(detail.contains("Pending"), "detail should mention Pending: {detail}");
            }
            other => panic!("expected Error::Diag(plan-transition), got {other:?}"),
        }
        let a = plan.entries.iter().find(|c| c.name == "a").unwrap();
        assert_eq!(a.status, Status::Done, "status must not be mutated on illegal edge");
    }

    #[test]
    fn transition_rejects_pending_to_done_skipping_in_progress() {
        let mut plan = plan_with_changes(vec![change("a", Status::Pending)]);
        let err = plan
            .transition("a", Status::Done)
            .expect_err("Pending -> Done is not a legal edge; must go via InProgress");
        match err {
            Error::Diag { code, .. } => assert_eq!(code, "plan-transition"),
            other => panic!("expected plan-transition diag, got {other:?}"),
        }
    }

    #[test]
    fn transition_missing_entry() {
        let mut plan = plan_with_changes(vec![change("foo", Status::Pending)]);
        let err =
            plan.transition("nonexistent", Status::InProgress).expect_err("missing entry must Err");
        match err {
            Error::Diag { code, detail } => {
                assert_eq!(code, "plan-entry-not-found");
                assert!(detail.contains("nonexistent"), "message should mention name: {detail}");
            }
            other => panic!("expected Error::Diag, got {other:?}"),
        }
    }

    #[test]
    fn lifecycle_pending_to_reviewed_ok() {
        let mut plan = plan_with_changes(vec![change("a", Status::Pending)]);
        assert_eq!(plan.lifecycle, Lifecycle::Pending);
        plan.transition_lifecycle(Lifecycle::Reviewed).expect("pending -> reviewed ok");
        assert_eq!(plan.lifecycle, Lifecycle::Reviewed);
    }

    #[test]
    fn lifecycle_reviewed_is_terminal() {
        let mut plan = plan_with_changes(vec![change("a", Status::Pending)]);
        plan.transition_lifecycle(Lifecycle::Reviewed).expect("first stamp ok");
        let err = plan
            .transition_lifecycle(Lifecycle::Reviewed)
            .expect_err("reviewed -> reviewed must Err");
        match err {
            Error::Diag { code, .. } => assert_eq!(code, "plan-lifecycle-transition"),
            other => panic!("expected plan-lifecycle-transition diag, got {other:?}"),
        }
    }

    #[test]
    fn lifecycle_rejects_pending_to_pending() {
        let mut plan = plan_with_changes(vec![change("a", Status::Pending)]);
        let err =
            plan.transition_lifecycle(Lifecycle::Pending).expect_err("pending -> pending must Err");
        match err {
            Error::Diag { code, .. } => assert_eq!(code, "plan-lifecycle-transition"),
            other => panic!("expected plan-lifecycle-transition diag, got {other:?}"),
        }
    }
}
