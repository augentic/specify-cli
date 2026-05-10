//! Status transition rules and `Plan::transition` lifecycle writer.
//!
//! `Plan::transition` is the single writer of [`Entry::status`] —
//! [`super::amend::Plan::amend`] and [`super::create::Plan::create`]
//! cannot mutate it. See `rfc-2-execution.md` §"Transition Rules" for
//! the canonical edge table.

use specify_error::Error;

use super::model::{Entry, Plan, Status};

impl Status {
    /// Every variant in declaration order. Used by exhaustive transition
    /// tests and dashboard counters that need to enumerate states.
    pub const ALL: [Self; 6] =
        [Self::Pending, Self::InProgress, Self::Done, Self::Blocked, Self::Failed, Self::Skipped];

    /// Whether `self -> target` is a legal edge in the plan-entry state
    /// machine. See `rfc-2-execution.md` §"Transition Rules" for the canonical
    /// table; the 10 edges enumerated below are the *only* legal ones.
    /// `Done` is terminal: every edge with `Done` on the left is `false`.
    #[must_use]
    pub const fn can_transition_to(&self, target: &Self) -> bool {
        use Status::{Blocked, Done, Failed, InProgress, Pending, Skipped};
        matches!(
            (self, target),
            (Pending, InProgress | Blocked | Skipped)
                | (InProgress, Done | Failed | Blocked)
                | (Blocked | Failed | Skipped, Pending)
                | (Failed, Skipped)
        )
    }

    /// Return `target` if the edge is legal, otherwise an
    /// `Error::PlanTransition` carrying both endpoints by their `Debug`
    /// representation.
    ///
    /// # Errors
    ///
    /// Errors with `Error::PlanTransition` when `self -> target` is not
    /// in the legal edge table.
    pub fn transition(&self, target: Self) -> Result<Self, Error> {
        if self.can_transition_to(&target) {
            Ok(target)
        } else {
            Err(Error::PlanTransition {
                from: format!("{self:?}"),
                to: format!("{target:?}"),
            })
        }
    }
}

impl Plan {
    /// Transition the named entry to `target`, recording `reason` in
    /// [`Entry::status_reason`] per the rules documented in
    /// `rfc-2-execution.md` §Fields.
    ///
    /// `reason` is only meaningful when `target` is one of
    /// `{Failed, Blocked, Skipped}`; passing `Some(_)` with any other
    /// target returns an `Error::Diag`. On a legal reason-less
    /// transition to `Pending`, `InProgress`, or `Done`,
    /// `status_reason` is cleared.
    ///
    /// # Errors
    ///
    /// Errors when the entry is missing, when the edge is illegal, or
    /// when `--reason` is supplied for a clean target.
    pub fn transition(
        &mut self, name: &str, target: Status, reason: Option<&str>,
    ) -> Result<(), Error> {
        let entry: &mut Entry =
            self.entries.iter_mut().find(|c| c.name == name).ok_or_else(|| Error::Diag {
                code: "plan-entry-not-found",
                detail: format!("no change named '{name}' in plan"),
            })?;

        let new_status = entry.status.transition(target)?;

        match target {
            Status::Failed | Status::Blocked | Status::Skipped => {
                if let Some(s) = reason {
                    entry.status_reason = Some(s.to_string());
                }
            }
            Status::Pending | Status::InProgress | Status::Done => {
                if reason.is_some() {
                    return Err(Error::Diag {
                        code: "plan-transition-reason-not-allowed",
                        detail: format!("--reason is not valid when transitioning to {target:?}"),
                    });
                }
                entry.status_reason = None;
            }
        }

        entry.status = new_status;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::super::test_support::{change, plan_with_changes};
    use super::*;

    /// The 10 legal edges from `rfc-2-execution.md` §"Transition Rules".
    /// Kept here (not on `Status`) so the production matcher and the
    /// test oracle are independent representations of the same table.
    fn allowed_edges() -> HashSet<(Status, Status)> {
        use Status::{Blocked, Done, Failed, InProgress, Pending, Skipped};
        let mut set = HashSet::new();
        set.insert((Pending, InProgress));
        set.insert((Pending, Blocked));
        set.insert((Pending, Skipped));
        set.insert((InProgress, Done));
        set.insert((InProgress, Failed));
        set.insert((InProgress, Blocked));
        set.insert((Blocked, Pending));
        set.insert((Failed, Pending));
        set.insert((Failed, Skipped));
        set.insert((Skipped, Pending));
        set
    }

    #[test]
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
        for &t in &Status::ALL {
            assert!(!Status::Done.can_transition_to(&t), "Done must not allow -> {t:?}");
        }
    }

    #[test]
    fn illegal_edges_rejected() {
        use Status::{Blocked, Done, Failed, InProgress, Pending, Skipped};
        let cases: &[(Status, Status)] = &[
            (Done, Pending),
            (Done, InProgress),
            (Done, Failed),
            (Pending, Done),
            (Pending, Failed),
            (Skipped, Failed),
            (InProgress, Pending),
            (InProgress, Skipped),
            (Blocked, Failed),
            (Pending, Pending),
            (InProgress, InProgress),
            (Done, Done),
            (Blocked, Blocked),
            (Failed, Failed),
            (Skipped, Skipped),
        ];

        for &(from, to) in cases {
            assert!(
                !from.can_transition_to(&to),
                "{from:?} -> {to:?} must be rejected by can_transition_to"
            );
            let err = from.transition(to).expect_err(&format!("{from:?} -> {to:?} should be Err"));
            match err {
                Error::PlanTransition { from: f, to: t } => {
                    assert_eq!(f, format!("{from:?}"), "from payload mismatch");
                    assert_eq!(t, format!("{to:?}"), "to payload mismatch");
                }
                other => panic!("expected Error::PlanTransition, got {other:?}"),
            }
        }
    }

    #[test]
    fn table_matches_oracle() {
        let allowed = allowed_edges();
        for &from in &Status::ALL {
            for &to in &Status::ALL {
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
            Error::PlanTransition { from, to } => {
                assert_eq!(from, "Done");
                assert_eq!(to, "Pending");
            }
            other => panic!("expected Error::PlanTransition, got {other:?}"),
        }
    }

    #[test]
    fn transition_clears_reason_on_reentry() {
        let mut plan = plan_with_changes(vec![Entry {
            name: "a".into(),
            project: Some("default".into()),
            capability: None,
            status: Status::Failed,
            depends_on: vec![],
            sources: vec![],
            context: vec![],
            description: None,
            status_reason: Some("crashed".into()),
        }]);
        plan.transition("a", Status::Pending, None).expect("failed -> pending ok");
        let a = plan.entries.iter().find(|c| c.name == "a").unwrap();
        assert_eq!(a.status, Status::Pending);
        assert_eq!(a.status_reason, None, "re-entry to Pending must clear status_reason");
    }

    #[test]
    fn transition_writes_reason() {
        let mut plan = plan_with_changes(vec![
            change("a", Status::Pending),
            change("b", Status::InProgress),
            change("c", Status::Failed),
        ]);

        plan.transition("a", Status::Blocked, Some("needs scope")).expect("pending -> blocked ok");
        let a = plan.entries.iter().find(|c| c.name == "a").unwrap();
        assert_eq!(a.status, Status::Blocked);
        assert_eq!(a.status_reason.as_deref(), Some("needs scope"));

        plan.transition("b", Status::Failed, Some("broken")).expect("in-progress -> failed ok");
        let b = plan.entries.iter().find(|c| c.name == "b").unwrap();
        assert_eq!(b.status, Status::Failed);
        assert_eq!(b.status_reason.as_deref(), Some("broken"));

        plan.transition("c", Status::Skipped, Some("abandoned")).expect("failed -> skipped ok");
        let c = plan.entries.iter().find(|c| c.name == "c").unwrap();
        assert_eq!(c.status, Status::Skipped);
        assert_eq!(c.status_reason.as_deref(), Some("abandoned"));
    }

    #[test]
    fn transition_rejects_reason_on_clean_target() {
        let mut plan =
            plan_with_changes(vec![change("a", Status::Pending), change("b", Status::InProgress)]);

        let err = plan
            .transition("a", Status::InProgress, Some("why"))
            .expect_err("reason on InProgress target must Err");
        match err {
            Error::Diag { code, detail } => {
                assert_eq!(code, "plan-transition-reason-not-allowed");
                assert!(detail.contains("--reason"), "message should mention --reason: {detail}");
            }
            other => panic!("expected Error::Diag, got {other:?}"),
        }
        let a = plan.entries.iter().find(|c| c.name == "a").unwrap();
        assert_eq!(a.status, Status::Pending, "a.status must be unchanged");

        let err = plan
            .transition("b", Status::Done, Some("why"))
            .expect_err("reason on Done target must Err");
        match err {
            Error::Diag { code, detail } => {
                assert_eq!(code, "plan-transition-reason-not-allowed");
                assert!(detail.contains("--reason"), "message should mention --reason: {detail}");
            }
            other => panic!("expected Error::Diag, got {other:?}"),
        }
        let b = plan.entries.iter().find(|c| c.name == "b").unwrap();
        assert_eq!(b.status, Status::InProgress, "b.status must be unchanged");
    }

    #[test]
    fn transition_rejects_illegal_edge() {
        let mut plan = plan_with_changes(vec![change("a", Status::Done)]);
        let err = plan
            .transition("a", Status::Pending, None)
            .expect_err("Done -> Pending must Err from state machine");
        match err {
            Error::PlanTransition { from, to } => {
                assert_eq!(from, "Done");
                assert_eq!(to, "Pending");
            }
            other => panic!("expected Error::PlanTransition, got {other:?}"),
        }
        let a = plan.entries.iter().find(|c| c.name == "a").unwrap();
        assert_eq!(a.status, Status::Done, "status must not be mutated on illegal edge");
    }

    #[test]
    fn transition_missing_entry() {
        let mut plan = plan_with_changes(vec![change("foo", Status::Pending)]);
        let err = plan
            .transition("nonexistent", Status::InProgress, None)
            .expect_err("missing entry must Err");
        match err {
            Error::Diag { code, detail } => {
                assert_eq!(code, "plan-entry-not-found");
                assert!(detail.contains("nonexistent"), "message should mention name: {detail}");
            }
            other => panic!("expected Error::Diag, got {other:?}"),
        }
    }

    #[test]
    fn status_display_matches_serde() {
        assert_eq!(Status::Pending.to_string(), "pending");
        assert_eq!(Status::InProgress.to_string(), "in-progress");
        assert_eq!(Status::Done.to_string(), "done");
        assert_eq!(Status::Blocked.to_string(), "blocked");
        assert_eq!(Status::Failed.to_string(), "failed");
        assert_eq!(Status::Skipped.to_string(), "skipped");
    }
}
