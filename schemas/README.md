# Specify CLI Schemas

This directory contains JSON Schemas and bundled workflow fixtures used by the `specify` CLI.

## CLI-owned schemas

| Schema | Purpose |
|---|---|
| [`capability.schema.json`](capability.schema.json) | Validates a Specify capability manifest (`capability.yaml`) per RFC-13 §Capability manifest and protocol. |
| [`brief/schema.json`](brief/schema.json) | Validates YAML frontmatter in capability brief markdown files. |
| [`codex-rule.schema.json`](codex-rule.schema.json) | Validates YAML frontmatter in codex rule markdown files. |
| [`plan/plan.schema.json`](plan/plan.schema.json) | Validates `plan.yaml` structure. |
| [`plan-validate-output/schema.json`](plan-validate-output/schema.json) | Validates `specify plan validate --format json` output. |
| [`cache-meta.schema.json`](cache-meta.schema.json) | Validates schema cache metadata written under `.specify/.cache`. |
| [`context-lock.schema.json`](context-lock.schema.json) | Validates `.specify/context.lock`, the sidecar used by `specify context` drift checks. |

## Bundled workflow schema

The CLI also carries a small [`omnia`](omnia/README.md) workflow schema fixture for tests and examples. The published Specify workflow schemas live in `augentic/specify` under `schemas/omnia`, `schemas/vectis`, and `schemas/contracts`.
