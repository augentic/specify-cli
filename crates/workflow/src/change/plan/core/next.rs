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
    /// workflow §CLI surface — `plan add` / `amend` write `Pending`
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
mod tests;
