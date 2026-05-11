//! [`Plan::amend`]: in-place edit of an existing entry's non-status fields.

use specify_error::Error;

use super::model::{EntryPatch, Plan, Severity};

impl Plan {
    /// Apply `patch` to the entry named `name`. `None` fields on the
    /// patch leave the corresponding [`Entry`](super::model::Entry) field
    /// unchanged; `Some(v)` replaces wholesale. `description` is
    /// three-way: `None` = leave, `Some(None)` = clear, `Some(Some(s))` =
    /// replace. `status` is intentionally not patchable — see
    /// [`EntryPatch`] and [`Plan::transition`] for the
    /// single-writer-for-status note.
    ///
    /// After mutation, the plan is re-validated. Any `Error`-level
    /// finding reverts the single-entry mutation (we snapshot the
    /// pre-mutation entry at the top of the function and write it
    /// back on failure) and returns an `Error::Diag`.
    ///
    /// `amend` does not consult `Entry::status` — it is legal to
    /// amend the currently-`in-progress` entry's non-status fields,
    /// per the execution spec's §"Phase Boundary → Rule 2".
    ///
    /// # Errors
    ///
    /// Errors when no entry matches `name` or when post-amend
    /// validation fails.
    pub fn amend(&mut self, name: &str, patch: EntryPatch) -> Result<(), Error> {
        let idx = self.entries.iter().position(|c| c.name == name).ok_or_else(|| Error::Diag {
            code: "plan-entry-not-found",
            detail: format!("no slice named '{name}' in plan"),
        })?;

        let snapshot = self.entries[idx].clone();

        {
            let entry = &mut self.entries[idx];
            if let Some(v) = patch.depends_on {
                entry.depends_on = v;
            }
            if let Some(v) = patch.sources {
                entry.sources = v;
            }
            if let Some(v) = patch.project {
                entry.project = v;
            }
            if let Some(v) = patch.capability {
                entry.capability = v;
            }
            if let Some(v) = patch.description {
                entry.description = v;
            }
            if let Some(v) = patch.context {
                entry.context = v;
            }
        }

        let errors: Vec<_> =
            self.validate(None, None).into_iter().filter(|r| r.level == Severity::Error).collect();
        if let Some(first) = errors.first() {
            let msg = first.message.clone();
            self.entries[idx] = snapshot;
            return Err(Error::Diag {
                code: "plan-amend-validation-failed",
                detail: format!("plan validation failed after amend: {msg}"),
            });
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::super::model::{Entry, Status};
    use super::super::test_support::{change, plan_with_changes};
    use super::*;

    #[test]
    fn amend_deps() {
        let mut plan = plan_with_changes(vec![change("a", Status::Pending), {
            let mut b = change("b", Status::Pending);
            b.depends_on = vec!["a".into()];
            b
        }]);
        let patch = EntryPatch {
            depends_on: Some(vec![]),
            ..EntryPatch::default()
        };
        plan.amend("b", patch).expect("amend ok");
        let b = plan.entries.iter().find(|c| c.name == "b").unwrap();
        assert!(b.depends_on.is_empty(), "depends_on should be replaced with empty vec");
    }

    #[test]
    fn amend_description_three_way() {
        let mut plan = plan_with_changes(vec![Entry {
            name: "foo".into(),
            project: Some("default".into()),
            capability: None,
            status: Status::Pending,
            depends_on: vec![],
            sources: vec![],
            context: vec![],
            description: Some("original".into()),
            status_reason: None,
        }]);

        plan.amend("foo", EntryPatch::default()).expect("amend none ok");
        assert_eq!(
            plan.entries[0].description.as_deref(),
            Some("original"),
            "None description must leave description unchanged"
        );

        plan.amend(
            "foo",
            EntryPatch {
                description: Some(None),
                ..EntryPatch::default()
            },
        )
        .expect("amend clear ok");
        assert_eq!(
            plan.entries[0].description, None,
            "Some(None) description must clear description"
        );

        plan.amend(
            "foo",
            EntryPatch {
                description: Some(Some("new".into())),
                ..EntryPatch::default()
            },
        )
        .expect("amend replace ok");
        assert_eq!(
            plan.entries[0].description.as_deref(),
            Some("new"),
            "Some(Some(s)) description must replace description"
        );
    }

    #[test]
    fn amend_leaves_unchanged() {
        let plan = Plan {
            name: "test".into(),
            sources: {
                let mut m = BTreeMap::new();
                m.insert("a".to_string(), "/path/a".to_string());
                m
            },
            entries: vec![
                Entry {
                    name: "foo".into(),
                    project: Some("default".into()),
                    capability: None,
                    status: Status::Pending,
                    depends_on: vec![],
                    sources: vec!["a".into()],
                    context: vec![],
                    description: Some("d".into()),
                    status_reason: None,
                },
                change("b", Status::Pending),
                change("x", Status::Pending),
            ],
        };
        let mut plan = plan;
        let patch = EntryPatch {
            depends_on: Some(vec!["x".into()]),
            ..EntryPatch::default()
        };
        plan.amend("foo", patch).expect("amend ok");
        let foo = plan.entries.iter().find(|c| c.name == "foo").unwrap();
        assert_eq!(foo.depends_on, vec!["x".to_string()]);
        assert_eq!(foo.sources, vec!["a".to_string()], "sources untouched");
        assert_eq!(foo.description.as_deref(), Some("d"), "description untouched");
    }

    #[test]
    fn amend_missing_entry() {
        let mut plan = plan_with_changes(vec![change("foo", Status::Pending)]);
        let err =
            plan.amend("nonexistent", EntryPatch::default()).expect_err("missing entry must Err");
        match err {
            Error::Diag { code, detail } => {
                assert_eq!(code, "plan-entry-not-found");
                assert!(
                    detail.contains("nonexistent"),
                    "message should mention name, got: {detail}"
                );
            }
            other => panic!("expected Error::Diag, got {other:?}"),
        }
    }

    #[test]
    fn amend_rejects_cycle() {
        let mut plan =
            plan_with_changes(vec![change("a", Status::Pending), change("b", Status::Pending)]);

        plan.amend(
            "a",
            EntryPatch {
                depends_on: Some(vec!["b".into()]),
                ..EntryPatch::default()
            },
        )
        .expect("a -> [b] is acyclic; amend ok");

        let err = plan
            .amend(
                "b",
                EntryPatch {
                    depends_on: Some(vec!["a".into()]),
                    ..EntryPatch::default()
                },
            )
            .expect_err("introducing cycle must Err");
        match err {
            Error::Diag { code, detail } => {
                assert_eq!(code, "plan-amend-validation-failed");
                assert!(
                    detail.contains("plan validation failed after amend"),
                    "expected amend rollback message, got: {detail}"
                );
            }
            other => panic!("expected Error::Diag, got {other:?}"),
        }

        let b = plan.entries.iter().find(|c| c.name == "b").unwrap();
        assert!(
            b.depends_on.is_empty(),
            "b.depends_on must be unchanged after failed amend, got {:?}",
            b.depends_on
        );
    }

    #[test]
    fn amend_project_three_way() {
        let mut plan = plan_with_changes(vec![Entry {
            name: "foo".into(),
            project: Some("alpha".into()),
            capability: None,
            status: Status::Pending,
            depends_on: vec![],
            sources: vec![],
            context: vec![],
            description: None,
            status_reason: None,
        }]);

        plan.amend("foo", EntryPatch::default()).expect("amend none ok");
        assert_eq!(
            plan.entries[0].project.as_deref(),
            Some("alpha"),
            "None must leave project unchanged"
        );

        plan.amend(
            "foo",
            EntryPatch {
                project: Some(Some("beta".into())),
                ..EntryPatch::default()
            },
        )
        .expect("amend replace ok");
        assert_eq!(
            plan.entries[0].project.as_deref(),
            Some("beta"),
            "Some(Some(s)) must replace project"
        );

        plan.amend(
            "foo",
            EntryPatch {
                project: Some(None),
                capability: Some(Some("contracts@v1".into())),
                ..EntryPatch::default()
            },
        )
        .expect("amend clear ok");
        assert_eq!(plan.entries[0].project, None, "Some(None) must clear project");
    }

    #[test]
    fn amend_capability_three_way() {
        let mut plan = plan_with_changes(vec![Entry {
            name: "foo".into(),
            project: Some("default".into()),
            capability: Some("omnia@v1".into()),
            status: Status::Pending,
            depends_on: vec![],
            sources: vec![],
            context: vec![],
            description: None,
            status_reason: None,
        }]);

        plan.amend("foo", EntryPatch::default()).expect("amend none ok");
        assert_eq!(
            plan.entries[0].capability.as_deref(),
            Some("omnia@v1"),
            "None must leave capability unchanged"
        );

        plan.amend(
            "foo",
            EntryPatch {
                capability: Some(Some("contracts@v1".into())),
                ..EntryPatch::default()
            },
        )
        .expect("amend replace ok");
        assert_eq!(
            plan.entries[0].capability.as_deref(),
            Some("contracts@v1"),
            "Some(Some(s)) must replace capability"
        );

        plan.amend(
            "foo",
            EntryPatch {
                capability: Some(None),
                ..EntryPatch::default()
            },
        )
        .expect("amend clear ok");
        assert_eq!(plan.entries[0].capability, None, "Some(None) must clear capability");
    }

    #[test]
    fn amend_context_replace() {
        let mut entry = change("foo", Status::Pending);
        entry.context = vec!["old/path.yaml".into()];
        let mut plan = plan_with_changes(vec![entry]);

        plan.amend(
            "foo",
            EntryPatch {
                context: Some(vec!["new/path.yaml".into(), "another.md".into()]),
                ..EntryPatch::default()
            },
        )
        .expect("amend ok");
        assert_eq!(
            plan.entries[0].context,
            vec!["new/path.yaml", "another.md"],
            "amend must replace context wholesale"
        );
    }

    #[test]
    fn amend_context_none_unchanged() {
        let mut entry = change("foo", Status::Pending);
        entry.context = vec!["keep/this.yaml".into()];
        let mut plan = plan_with_changes(vec![entry]);

        plan.amend("foo", EntryPatch::default()).expect("amend ok");
        assert_eq!(
            plan.entries[0].context,
            vec!["keep/this.yaml"],
            "None context must leave field unchanged"
        );
    }
}
