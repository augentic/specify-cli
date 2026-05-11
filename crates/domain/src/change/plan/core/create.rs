//! Plan scaffolding: [`Plan::init`] for an empty plan and
//! [`Plan::create`] for a single-entry append.

use std::collections::BTreeMap;

use specify_error::Error;
use crate::slice::actions::validate_name;

use super::model::{Entry, Plan, Severity, Status};

impl Plan {
    /// Create an empty plan with the given name and optional named sources.
    ///
    /// Every entry starts with `status: pending`; this just initialises the
    /// top-level struct. The name is validated with
    /// [`crate::slice::actions::validate_name`] so it obeys the same kebab-case
    /// rules as change names.
    ///
    /// Does NOT write anything to disk. Call [`Plan::save`] afterwards.
    ///
    /// # Errors
    ///
    /// Errors when `name` is not kebab-case.
    pub fn init(name: &str, sources: BTreeMap<String, String>) -> Result<Self, Error> {
        validate_name(name)?;
        Ok(Self {
            name: name.to_string(),
            sources,
            entries: vec![],
        })
    }

    /// Append a new entry to the plan, rejecting duplicate names and
    /// invalid kebab-case names. The incoming `status` is forced to
    /// [`Status::Pending`] (and `status_reason` cleared) so that
    /// creation cannot introduce a pre-occupied lifecycle state — the
    /// single-writer-for-status invariant documented in
    /// [`Plan::transition`].
    ///
    /// After mutation, the plan is re-validated. Any `Error`-level
    /// finding (unknown `depends_on`/`sources`, cycle introduced by the
    /// new entry, etc.) rolls back the append and returns an
    /// `Error::Diag`. Warnings are tolerated — they're a CLI concern,
    /// not a library-level hard stop.
    ///
    /// # Errors
    ///
    /// Errors when the name is invalid, when an entry with the same
    /// name already exists, or when post-append validation fails.
    pub fn create(&mut self, change: Entry) -> Result<(), Error> {
        validate_name(&change.name)?;

        if self.entries.iter().any(|c| c.name == change.name) {
            return Err(Error::Diag {
                code: "plan-entry-duplicate-name",
                detail: format!("plan already contains a change named '{}'", change.name),
            });
        }

        let mut change = change;
        change.status = Status::Pending;
        change.status_reason = None;

        self.entries.push(change);
        let errors: Vec<_> =
            self.validate(None, None).into_iter().filter(|r| r.level == Severity::Error).collect();
        if let Some(first) = errors.first() {
            let msg = first.message.clone();
            self.entries.pop();
            return Err(Error::Diag {
                code: "plan-create-validation-failed",
                detail: format!("plan validation failed after create: {msg}"),
            });
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{change, change_with_deps, plan_with_changes};
    use super::*;

    #[test]
    fn create_forces_pending_clears_reason() {
        let mut plan = plan_with_changes(vec![]);
        let incoming = Entry {
            name: "foo".into(),
            project: Some("default".into()),
            capability: None,
            status: Status::Failed,
            depends_on: vec![],
            sources: vec![],
            context: vec![],
            description: None,
            status_reason: Some("bogus".into()),
        };
        plan.create(incoming).expect("create ok");
        assert_eq!(plan.entries.len(), 1);
        assert_eq!(plan.entries[0].name, "foo");
        assert_eq!(
            plan.entries[0].status,
            Status::Pending,
            "create must force status to Pending regardless of input"
        );
        assert_eq!(
            plan.entries[0].status_reason, None,
            "create must clear status_reason regardless of input"
        );
    }

    #[test]
    fn create_rejects_duplicate() {
        let mut plan = plan_with_changes(vec![change("foo", Status::Pending)]);
        let dup = change("foo", Status::Pending);
        let err = plan.create(dup).expect_err("duplicate must be rejected");
        match err {
            Error::Diag { code, detail } => {
                assert_eq!(code, "plan-entry-duplicate-name");
                assert!(
                    detail.contains("already contains") && detail.contains("foo"),
                    "unexpected message: {detail}"
                );
            }
            other => panic!("expected Error::Diag, got {other:?}"),
        }
        assert_eq!(plan.entries.len(), 1, "plan must still have exactly one entry");
    }

    #[test]
    fn create_rejects_bad_name() {
        let mut plan = plan_with_changes(vec![]);
        let bad = change("Bad-Name", Status::Pending);
        let err = plan.create(bad).expect_err("invalid name must be rejected");
        match err {
            Error::InvalidName(msg) => {
                assert!(msg.contains("kebab-case"), "expected kebab-case in message, got: {msg}");
            }
            other => panic!("expected Error::InvalidName, got {other:?}"),
        }
        assert!(plan.entries.is_empty(), "plan must remain untouched after invalid name");
    }

    #[test]
    fn create_rejects_unknown_depends_on() {
        let mut plan = plan_with_changes(vec![
            change("a", Status::Pending),
            change_with_deps("b", Status::Pending, &["a"]),
        ]);
        let c = change_with_deps("c", Status::Pending, &["does-not-exist"]);
        let err = plan.create(c).expect_err("unknown depends-on must roll back");
        match err {
            Error::Diag { code, detail } => {
                assert_eq!(code, "plan-create-validation-failed");
                assert!(
                    detail.contains("plan validation failed after create"),
                    "rollback message missing, got: {detail}"
                );
            }
            other => panic!("expected Error::Diag, got {other:?}"),
        }
        assert_eq!(plan.entries.len(), 2, "plan must still have only its original entries");
        let names: Vec<&str> = plan.entries.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["a", "b"], "existing entries must be untouched");
    }

    #[test]
    fn create_rolls_back_on_failure() {
        let mut plan = plan_with_changes(vec![change("foo", Status::Pending)]);
        let bar = change_with_deps("bar", Status::Pending, &["nonexistent"]);
        let err = plan.create(bar).expect_err("must Err");
        assert!(matches!(err, Error::Diag { code, .. } if code == "plan-create-validation-failed"));
        assert_eq!(plan.entries.len(), 1, "plan length unchanged after rollback");
        assert_eq!(plan.entries[0].name, "foo");
        assert_eq!(plan.entries[0].status, Status::Pending);
        assert!(plan.entries[0].depends_on.is_empty());
    }

    #[test]
    fn create_rejects_no_project_or_capability() {
        let mut plan = plan_with_changes(vec![]);
        let entry = Entry {
            name: "bad".into(),
            project: None,
            capability: None,
            status: Status::Pending,
            depends_on: vec![],
            sources: vec![],
            context: vec![],
            description: None,
            status_reason: None,
        };
        let err = plan.create(entry).expect_err("must reject entry without project or capability");
        match err {
            Error::Diag { code, detail } => {
                assert_eq!(code, "plan-create-validation-failed");
                assert!(
                    detail.contains("project") && detail.contains("capability"),
                    "error should mention project and capability: {detail}"
                );
            }
            other => panic!("expected Error::Diag, got {other:?}"),
        }
        assert!(plan.entries.is_empty(), "plan must remain empty after rejected create");
    }

    #[test]
    fn create_preserves_context() {
        let mut plan = plan_with_changes(vec![]);
        let entry = Entry {
            name: "with-ctx".into(),
            project: Some("default".into()),
            capability: None,
            status: Status::Pending,
            depends_on: vec![],
            sources: vec![],
            context: vec!["contracts/http/foo.yaml".into()],
            description: None,
            status_reason: None,
        };
        plan.create(entry).expect("create ok");
        assert_eq!(
            plan.entries[0].context,
            vec!["contracts/http/foo.yaml"],
            "create must preserve context"
        );
    }

    #[test]
    fn create_rejects_bad_context() {
        let mut plan = plan_with_changes(vec![]);
        let entry = Entry {
            name: "bad-ctx".into(),
            project: Some("default".into()),
            capability: None,
            status: Status::Pending,
            depends_on: vec![],
            sources: vec![],
            context: vec!["../escape".into()],
            description: None,
            status_reason: None,
        };
        let err = plan.create(entry).expect_err("invalid context path must be rejected");
        match err {
            Error::Diag { code, detail } => {
                assert_eq!(code, "plan-create-validation-failed");
                assert!(
                    detail.contains("context-path-invalid") || detail.contains(".."),
                    "error should mention context path issue, got: {detail}"
                );
            }
            other => panic!("expected Error::Diag, got {other:?}"),
        }
        assert!(plan.entries.is_empty(), "rollback must remove the entry");
    }

    #[test]
    fn init_empty_plan() {
        let plan = Plan::init("platform-v2", BTreeMap::new()).expect("init ok");
        assert_eq!(plan.name, "platform-v2");
        assert!(plan.sources.is_empty(), "sources should default to empty");
        assert!(plan.entries.is_empty(), "changes should default to empty");
    }

    #[test]
    fn init_preserves_sources() {
        let mut sources = BTreeMap::new();
        sources.insert("monolith".to_string(), "/path/to/legacy".to_string());
        sources.insert("orders".to_string(), "git@github.com:org/orders.git".to_string());
        sources.insert("payments".to_string(), "git@github.com:org/payments.git".to_string());

        let plan = Plan::init("big", sources.clone()).expect("init ok");
        assert_eq!(plan.sources, sources, "init must preserve the sources map verbatim");
        assert_eq!(plan.sources.len(), 3);
    }

    #[test]
    fn init_rejects_bad_name() {
        let err = Plan::init("BAD_NAME", BTreeMap::new()).expect_err("invalid name must Err");
        match err {
            Error::InvalidName(msg) => {
                assert!(msg.contains("kebab-case"), "expected kebab-case in message, got: {msg}");
            }
            other => panic!("expected Error::InvalidName, got {other:?}"),
        }
    }

    #[test]
    fn init_accepts_kebab_case() {
        let plan = Plan::init("a-b-c", BTreeMap::new()).expect("kebab name accepted");
        assert_eq!(plan.name, "a-b-c");
    }

    #[test]
    fn init_validates() {
        let plan = Plan::init("foo", BTreeMap::new()).expect("init ok");
        let findings = plan.validate(None, None);
        assert!(
            findings.is_empty(),
            "freshly-scaffolded plan must pass validation, got: {findings:#?}"
        );
    }
}
