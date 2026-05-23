//! [`Plan::next_eligible`] (single-step scheduler).

use std::collections::HashMap;

use specify_error::Error;

use super::model::{Entry, Plan, Status};

impl Plan {
    /// First entry in list order whose dependencies are all `done` and
    /// whose own status is `pending`. Returns `None` when nothing is
    /// eligible (plan finished, blocked, empty) **or when any entry is
    /// currently `in-progress`** — the driver must not pick a new
    /// change while one is active. The in-progress check runs before
    /// dependency eligibility checks.
    ///
    /// An unknown `depends_on` target is treated as "not done", so the
    /// entry is not eligible. Orphan-reference diagnostics belong to
    /// [`Plan::validate`].
    #[must_use]
    pub fn next_eligible(&self) -> Option<&Entry> {
        if self.entries.iter().any(|c| c.status == Status::InProgress) {
            return None;
        }
        let status_by_name: HashMap<&str, Status> =
            self.entries.iter().map(|c| (c.name.as_str(), c.status)).collect();
        self.entries.iter().find(|c| {
            c.status == Status::Pending
                && c.depends_on
                    .iter()
                    .all(|dep| status_by_name.get(dep.as_str()).copied() == Some(Status::Done))
        })
    }

    /// Atomically advance the plan: if there is no active in-progress
    /// entry, transition the next eligible `Pending` entry to
    /// `InProgress` and return it; otherwise return the existing
    /// active entry without writing anything.
    ///
    /// This is the **only** writer of per-entry `InProgress` per
    /// RFC-25 §CLI surface — `plan add` / `amend` write `Pending`
    /// only, and `plan transition` writes `Done` only.
    ///
    /// Returns `None` when the plan is drained (no active and no
    /// eligible pending entry).
    ///
    /// # Errors
    ///
    /// Errors when the underlying state transition is illegal —
    /// in practice unreachable since `next_eligible` filters for
    /// `Pending` entries and the only legal edge from `Pending` is
    /// `→ InProgress`.
    pub fn advance_next(&mut self) -> Result<Option<&Entry>, Error> {
        if self.is_executing() {
            return Ok(self.entries.iter().find(|e| e.status == Status::InProgress));
        }
        let Some(name) = self.next_eligible().map(|e| e.name.clone()) else {
            return Ok(None);
        };
        self.transition(&name, Status::InProgress)?;
        Ok(self.entries.iter().find(|e| e.name == name))
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{
        RFC_EXAMPLE_YAML, change, change_with_deps, plan_with_changes,
    };
    use super::*;

    #[test]
    fn next_eligible_picks_first_ready() {
        let plan = plan_with_changes(vec![
            change("a", Status::Done),
            change("b", Status::Done),
            change_with_deps("c", Status::Pending, &["b"]),
        ]);
        let eligible = plan.next_eligible().expect("c should be eligible");
        assert_eq!(eligible.name, "c");
    }

    #[test]
    fn next_eligible_skips_unmet_deps() {
        let plan = plan_with_changes(vec![
            change("a", Status::Pending),
            change_with_deps("b", Status::Pending, &["a"]),
        ]);
        let eligible = plan.next_eligible().expect("a should be eligible");
        assert_eq!(eligible.name, "a", "b's dep 'a' is not done, so a (no deps) wins");
    }

    #[test]
    fn next_eligible_blocked_by_in_progress() {
        let plan =
            plan_with_changes(vec![change("a", Status::InProgress), change("b", Status::Pending)]);
        assert!(
            plan.next_eligible().is_none(),
            "an in-progress entry must block any new selection"
        );
    }

    #[test]
    fn next_eligible_none_when_finished() {
        // Post-RFC-25 the only terminal per-entry state is `Done`. A
        // plan whose entries are all `Done` is drained — `next_eligible`
        // must report nothing.
        let plan = plan_with_changes(vec![
            change("a", Status::Done),
            change("b", Status::Done),
            change("c", Status::Done),
        ]);
        assert!(plan.next_eligible().is_none());
    }

    #[test]
    fn next_eligible_tiebreak() {
        let plan = plan_with_changes(vec![
            change("alpha", Status::Pending),
            change("beta", Status::Pending),
        ]);
        let eligible = plan.next_eligible().expect("alpha should be first");
        assert_eq!(eligible.name, "alpha", "list-order tie-break must pick the first entry");
    }

    /// Drive `next_eligible` forward across the reference example plan,
    /// marking each returned entry `done`, and assert the exact
    /// traversal sequence.
    #[test]
    fn next_eligible_rfc_forward() {
        let mut plan: Plan = serde_saphyr::from_str(RFC_EXAMPLE_YAML).expect("parse rfc fixture");
        for entry in &mut plan.entries {
            entry.status = Status::Pending;
        }

        let mut traversal = Vec::new();
        while let Some(next) = plan.next_eligible() {
            let name = next.name.clone();
            traversal.push(name.clone());
            let entry = plan
                .entries
                .iter_mut()
                .find(|c| c.name == name)
                .expect("returned name must exist in plan");
            entry.status = Status::Done;
        }

        let expected = [
            "user-registration",
            "email-verification",
            "registration-duplicate-email-crash",
            "notification-preferences",
            "extract-shared-validation",
            "product-catalog",
            "shopping-cart",
            "checkout-api",
            "checkout-ui",
        ];
        assert_eq!(
            traversal, expected,
            "next_eligible traversal should follow the §The Plan reference forward order"
        );
    }

    #[test]
    fn next_eligible_blocks_mid_cycle() {
        let plan = plan_with_changes(vec![
            change("in-flight", Status::InProgress),
            change_with_deps("a", Status::Pending, &["b"]),
            change_with_deps("b", Status::Pending, &["a"]),
        ]);
        assert!(
            plan.next_eligible().is_none(),
            "in-progress entry must block selection before any dependency walk"
        );
    }

    #[test]
    fn advance_next_writes_in_progress_then_returns_existing_active() {
        let mut plan = plan_with_changes(vec![
            change("a", Status::Pending),
            change_with_deps("b", Status::Pending, &["a"]),
        ]);
        // First call transitions `a` to in-progress and returns it.
        let active = plan.advance_next().expect("advance ok");
        assert_eq!(active.unwrap().name, "a");
        assert_eq!(plan.entries[0].status, Status::InProgress);
        // Subsequent calls report the existing active entry without
        // moving any other entry.
        let again = plan.advance_next().expect("advance ok");
        assert_eq!(again.unwrap().name, "a", "active entry must be returned, not advanced past");
        assert_eq!(plan.entries[0].status, Status::InProgress);
        assert_eq!(plan.entries[1].status, Status::Pending);
    }

    #[test]
    fn advance_next_reports_drained_when_all_done() {
        let mut plan =
            plan_with_changes(vec![change("a", Status::Done), change("b", Status::Done)]);
        let next = plan.advance_next().expect("advance ok");
        assert!(next.is_none(), "drained plan must report None");
        assert!(plan.is_drained());
    }
}
