//! `Plan::transition` and `Plan::transition_lifecycle` — the only
//! writers of `Entry::status` for `done` and `Plan::lifecycle` for
//! `approved`. Post-2.0 the legal edges are `Pending → InProgress`
//! (written by `plan next`, never here) and `InProgress → Done` per
//! entry, plus `Pending → Reviewed` plan-level (operator stamp at
//! Gate 1, workflow §The plan gate; `/spec:plan` MUST NOT call it).

use specify_error::Error;

use super::model::{Entry, Lifecycle, Plan, Status};

impl Plan {
    /// Transition the named entry to `target` (in practice always
    /// [`Status::Done`]; `Pending → InProgress` is reserved for
    /// `Plan::next`).
    ///
    /// # Errors
    /// `plan-entry-not-found` / `plan-transition` — see module docs.
    pub fn transition(&mut self, name: &str, target: Status) -> Result<(), Error> {
        let entry: &mut Entry =
            self.entries.iter_mut().find(|c| c.name == name).ok_or_else(|| Error::Diag {
                code: "plan-entry-not-found",
                detail: format!("no slice named '{name}' in plan"),
            })?;
        let current = entry.status;
        if matches!(
            (current, target),
            (Status::Pending, Status::InProgress) | (Status::InProgress, Status::Done)
        ) {
            entry.status = target;
            Ok(())
        } else {
            Err(Error::Diag {
                code: "plan-transition",
                detail: format!("cannot transition from {current:?} to {target:?}"),
            })
        }
    }

    /// Transition [`Plan::lifecycle`] to `target` — see module docs.
    ///
    /// # Errors
    /// `plan-lifecycle-transition` when the edge is not legal.
    pub fn transition_lifecycle(&mut self, target: Lifecycle) -> Result<(), Error> {
        let current = self.lifecycle;
        if matches!((current, target), (Lifecycle::Pending, Lifecycle::Approved)) {
            self.lifecycle = target;
            Ok(())
        } else {
            Err(Error::Diag {
                code: "plan-lifecycle-transition",
                detail: format!("cannot transition plan lifecycle from {current:?} to {target:?}"),
            })
        }
    }

    /// Walk a single entry one step backwards along the legal v1
    /// lifecycle (`Done → InProgress`, `InProgress → Pending`) and
    /// return `(from, to)` so the caller can emit the matching
    /// `plan.transition.undone` journal event.
    ///
    /// The undo verb refuses to skip rungs — `Done → Pending` MUST
    /// run twice so the journal records each rung independently and
    /// the operator never lands in a state the forward path cannot
    /// reach. `Pending` has no predecessor (no prior status to
    /// reinstate); `plan add` / `plan amend` are the only writers
    /// of `Pending`.
    ///
    /// # Errors
    ///
    /// - `plan-entry-not-found` when no entry on `self` matches
    ///   `name`.
    /// - `plan-transition-undo` when the entry is already at
    ///   `Pending` (nothing to undo).
    pub fn transition_undo(&mut self, name: &str) -> Result<(Status, Status), Error> {
        let entry: &mut Entry =
            self.entries.iter_mut().find(|c| c.name == name).ok_or_else(|| Error::Diag {
                code: "plan-entry-not-found",
                detail: format!("no slice named '{name}' in plan"),
            })?;
        let from = entry.status;
        let to = match from {
            Status::Done => Status::InProgress,
            Status::InProgress => Status::Pending,
            Status::Pending => {
                return Err(Error::Diag {
                    code: "plan-transition-undo",
                    detail: format!(
                        "cannot undo from `pending` on slice `{name}`; `pending` is the entry \
                         point and has no prior status to reinstate"
                    ),
                });
            }
        };
        entry.status = to;
        Ok((from, to))
    }
}

#[cfg(test)]
mod tests {
    use super::super::{change, plan_with_changes};
    use super::*;

    #[test]
    fn transition_in_progress_to_done() {
        let mut plan =
            plan_with_changes(vec![change("a", Status::Pending), change("b", Status::InProgress)]);
        plan.transition("b", Status::Done).expect("in-progress -> done ok");
        assert_eq!(plan.entries.iter().find(|c| c.name == "b").unwrap().status, Status::Done);
        let Err(Error::Diag { code, detail }) = plan.transition("a", Status::Done) else {
            panic!("Pending -> Done must Err with plan-transition diag");
        };
        assert_eq!(code, "plan-transition");
        assert!(detail.contains("Pending") && detail.contains("Done"), "endpoints in: {detail:?}");
        assert_eq!(plan.entries[0].status, Status::Pending, "status not mutated on illegal edge");
    }

    #[test]
    fn lifecycle_pending_to_approved_then_terminal() {
        let mut plan = plan_with_changes(vec![change("a", Status::Pending)]);
        plan.transition_lifecycle(Lifecycle::Approved).expect("pending -> approved ok");
        assert_eq!(plan.lifecycle, Lifecycle::Approved);
        let Err(Error::Diag { code, detail }) = plan.transition_lifecycle(Lifecycle::Approved)
        else {
            panic!("approved -> approved must Err");
        };
        assert_eq!(code, "plan-lifecycle-transition");
        assert!(detail.contains("Approved"), "endpoint in: {detail:?}");
    }

    #[test]
    fn undo_walks_status_backwards_one_rung_at_a_time() {
        let mut plan = plan_with_changes(vec![change("slice", Status::Done)]);
        let (from, to) = plan.transition_undo("slice").expect("done -> in-progress ok");
        assert_eq!((from, to), (Status::Done, Status::InProgress));
        assert_eq!(plan.entries[0].status, Status::InProgress);

        let (from, to) = plan.transition_undo("slice").expect("in-progress -> pending ok");
        assert_eq!((from, to), (Status::InProgress, Status::Pending));
        assert_eq!(plan.entries[0].status, Status::Pending);
    }

    #[test]
    fn undo_refuses_from_pending() {
        let mut plan = plan_with_changes(vec![change("slice", Status::Pending)]);
        let Err(Error::Diag { code, detail }) = plan.transition_undo("slice") else {
            panic!("undo from pending must Err with plan-transition-undo diag");
        };
        assert_eq!(code, "plan-transition-undo");
        assert!(detail.contains("pending"), "endpoint in: {detail:?}");
        assert_eq!(plan.entries[0].status, Status::Pending, "status not mutated on illegal undo");
    }

    #[test]
    fn undo_unknown_entry_diag() {
        let mut plan = plan_with_changes(vec![change("known", Status::InProgress)]);
        let Err(Error::Diag { code, .. }) = plan.transition_undo("ghost") else {
            panic!("unknown entry must Err with plan-entry-not-found");
        };
        assert_eq!(code, "plan-entry-not-found");
    }

    #[test]
    fn init_then_approved_models_auto_approve_at_create() {
        // auto-approve Gate-1 contract: `--auto-review` composes `Plan::init` with
        // `Plan::transition_lifecycle(Reviewed)` before the single
        // atomic save. The resulting in-memory plan must carry
        // `lifecycle: approved` so the post-init `Plan::save` writes
        // `lifecycle: approved` directly with no transient `pending`
        // round trip through disk.
        let mut plan =
            Plan::init("fresh", std::collections::BTreeMap::new()).expect("init fresh ok");
        assert_eq!(plan.lifecycle, Lifecycle::Pending, "fresh init defaults to pending");
        plan.transition_lifecycle(Lifecycle::Approved)
            .expect("--auto-review composes init + lifecycle stamp");
        assert_eq!(
            plan.lifecycle,
            Lifecycle::Approved,
            "in-memory plan must carry approved before save under --auto-review"
        );
    }
}
