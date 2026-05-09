---
id: OMNIA-004
title: Provider Boundary
severity: important
trigger: Runtime integration code bypasses an injected Omnia provider.
applicability:
  capabilities: [omnia@v1]
  languages: [rust]
  artifacts: [code, tests]
  paths:
    - crates/**/*.rs
review_mode: hybrid
deterministic_hints:
  - kind: path-pattern
    value: crates/**/*.rs
    description: Generated Rust crates live under the crate source tree.
  - kind: regex
    value: 'unwrap\('
references:
  - label: Omnia provider guardrails
    path: plugins/omnia/references/guardrails.md
  - label: Rust API Guidelines
    url: https://rust-lang.github.io/api-guidelines/
deprecated:
  reason: Kept as a fixture for validating deprecation metadata.
  replaced_by: RUST-003
---

## Rule

Use the injected provider interface for runtime integration instead of reaching around it.
