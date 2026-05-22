//! `Plan::transition` and `Plan::transition_lifecycle` — the only
//! writers of `Entry::status` for `done` and `Plan::lifecycle` for
//! `reviewed`. Post-RFC-25 the legal edges are `Pending → InProgress`
//! (written by `plan next`, never here) and `InProgress → Done` per
//! entry, plus `Pending → Reviewed` plan-level (operator stamp at
//! Gate 1, RFC-25 §The plan gate; `/spec:plan` MUST NOT call it).

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
        if matches!((current, target), (Lifecycle::Pending, Lifecycle::Reviewed)) {
            self.lifecycle = target;
            Ok(())
        } else {
            Err(Error::Diag {
                code: "plan-lifecycle-transition",
                detail: format!("cannot transition plan lifecycle from {current:?} to {target:?}"),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{change, plan_with_changes};
    use super::*;

    #[test]
    fn transition_in_progress_to_done_and_rejects_pending_skip() {
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
    fn lifecycle_pending_to_reviewed_then_terminal() {
        let mut plan = plan_with_changes(vec![change("a", Status::Pending)]);
        plan.transition_lifecycle(Lifecycle::Reviewed).expect("pending -> reviewed ok");
        assert_eq!(plan.lifecycle, Lifecycle::Reviewed);
        let Err(Error::Diag { code, detail }) = plan.transition_lifecycle(Lifecycle::Reviewed)
        else {
            panic!("reviewed -> reviewed must Err");
        };
        assert_eq!(code, "plan-lifecycle-transition");
        assert!(detail.contains("Reviewed"), "endpoint in: {detail:?}");
    }

    #[test]
    fn init_then_reviewed_models_auto_review_at_create() {
        // RFC-27 §D7: `--auto-review` composes `Plan::init` with
        // `Plan::transition_lifecycle(Reviewed)` before the single
        // atomic save. The resulting in-memory plan must carry
        // `lifecycle: reviewed` so the post-init `Plan::save` writes
        // `lifecycle: reviewed` directly with no transient `pending`
        // round trip through disk.
        let mut plan =
            Plan::init("fresh", std::collections::BTreeMap::new()).expect("init fresh ok");
        assert_eq!(plan.lifecycle, Lifecycle::Pending, "fresh init defaults to pending");
        plan.transition_lifecycle(Lifecycle::Reviewed)
            .expect("--auto-review composes init + lifecycle stamp");
        assert_eq!(
            plan.lifecycle,
            Lifecycle::Reviewed,
            "in-memory plan must carry reviewed before save under --auto-review"
        );
    }
}
