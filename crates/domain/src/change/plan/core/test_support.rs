//! Shared `#[cfg(test)]` fixtures used by every `core/` submodule.

use std::collections::BTreeMap;

use super::model::{Entry, Lifecycle, Plan, SliceAuthorityOverride, Status};

/// Reduced-state reproduction of the `rfc-2-execution.md` §"The Plan"
/// fixture. Per RFC-25, v1 has no per-entry `failed`, `blocked`, or
/// `skipped` state — entries either move forward or stay where they
/// are. The fixture has been mechanically rewritten to use the
/// surviving three-state enum.
pub(super) const RFC_EXAMPLE_YAML: &str = r"name: platform-v2
sources:
  monolith:
    adapter: code-typescript
    path: /path/to/legacy-codebase
  orders:
    adapter: code-typescript
    path: git@github.com:org/orders-service.git
  payments:
    adapter: code-typescript
    path: git@github.com:org/payments-service.git
  frontend:
    adapter: code-typescript
    path: git@github.com:org/web-app.git
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
    status: pending
  - name: checkout-ui
    project: platform
    sources: [frontend]
    depends-on: [checkout-api]
    status: pending
";

pub(super) fn plan_with_changes(changes: Vec<Entry>) -> Plan {
    Plan {
        name: "test".into(),
        lifecycle: Lifecycle::Pending,
        sources: BTreeMap::new(),
        entries: changes,
    }
}

pub(super) fn change(name: &str, status: Status) -> Entry {
    Entry {
        name: name.into(),
        project: Some("default".into()),
        target: None,
        status,
        depends_on: vec![],
        sources: vec![],
        context: vec![],
        description: None,
        divergence: None,
        authority_override: SliceAuthorityOverride::default(),
    }
}

pub(super) fn change_with_deps(name: &str, status: Status, deps: &[&str]) -> Entry {
    Entry {
        name: name.into(),
        project: Some("default".into()),
        target: None,
        status,
        depends_on: deps.iter().map(|s| (*s).to_string()).collect(),
        sources: vec![],
        context: vec![],
        description: None,
        divergence: None,
        authority_override: SliceAuthorityOverride::default(),
    }
}
