//! Shared `#[cfg(test)]` fixtures used by every `core/` submodule.

use std::collections::BTreeMap;

use super::model::{Entry, Plan, Status};

/// Verbatim reproduction of the `rfc-2-execution.md` §"The Plan"
/// fixture, used by model, io, validate, and next-selector tests.
pub(super) const RFC_EXAMPLE_YAML: &str = r"name: platform-v2
sources:
  monolith: /path/to/legacy-codebase
  orders: git@github.com:org/orders-service.git
  payments: git@github.com:org/payments-service.git
  frontend: git@github.com:org/web-app.git
slices:
  - name: user-registration
    project: platform
    sources: [monolith]
    status: done
  - name: email-verification
    project: platform
    sources: [monolith]
    depends-on: [user-registration]
    status: in-progress
  - name: registration-duplicate-email-crash
    project: platform
    description: >
      Duplicate email submission returns 500 instead of 409.
      Discovered during email-verification extraction.
    status: pending
  - name: notification-preferences
    project: platform
    depends-on: [user-registration]
    description: >
      Greenfield — user-facing notification channel and frequency settings.
    status: pending
  - name: extract-shared-validation
    project: platform
    description: >
      Pull duplicated input validation into a shared validation crate
      before building checkout-flow.
    depends-on: [email-verification]
    status: pending
  - name: product-catalog
    project: platform
    sources: [monolith]
    depends-on: [extract-shared-validation]
    status: pending
  - name: shopping-cart
    project: platform
    sources: [orders]
    depends-on: [product-catalog, user-registration]
    status: pending
  - name: checkout-api
    project: platform
    sources: [payments]
    depends-on: [shopping-cart]
    status: failed
    status-reason: >
      Type mismatch between cart line-item schema and payment gateway contract.
      Needs design revision after shopping-cart specs are updated.
  - name: checkout-ui
    project: platform
    sources: [frontend]
    depends-on: [checkout-api]
    status: pending
";

pub(super) fn plan_with_changes(changes: Vec<Entry>) -> Plan {
    Plan {
        name: "test".into(),
        sources: BTreeMap::new(),
        entries: changes,
    }
}

pub(super) fn change(name: &str, status: Status) -> Entry {
    Entry {
        name: name.into(),
        project: Some("default".into()),
        capability: None,
        status,
        depends_on: vec![],
        sources: vec![],
        context: vec![],
        description: None,
        status_reason: None,
    }
}

pub(super) fn change_with_deps(name: &str, status: Status, deps: &[&str]) -> Entry {
    Entry {
        name: name.into(),
        project: Some("default".into()),
        capability: None,
        status,
        depends_on: deps.iter().map(|s| (*s).to_string()).collect(),
        sources: vec![],
        context: vec![],
        description: None,
        status_reason: None,
    }
}
