# Specify CLI Schemas

This directory contains JSON Schemas and bundled workflow fixtures used by the `specify` CLI.

## CLI-owned schemas

| Schema | Purpose |
|---|---|
| [`schema.schema.json`](schema.schema.json) | Validates `schema.yaml`, the manifest for a selectable Specify workflow schema. |
| [`brief/schema.json`](brief/schema.json) | Validates YAML frontmatter in schema brief markdown files. |
| [`plan/plan.schema.json`](plan/plan.schema.json) | Validates `.specify/plan.yaml` structure. |
| [`plan-validate-output/schema.json`](plan-validate-output/schema.json) | Validates `specify plan validate --format json` output. |
| [`cache-meta.schema.json`](cache-meta.schema.json) | Validates schema cache metadata written under `.specify/.cache`. |

## Bundled workflow schema

The CLI also carries a small [`omnia`](omnia/README.md) workflow schema fixture for tests and examples. The published Specify workflow schemas live in `augentic/specify` under `schemas/omnia`, `schemas/vectis`, and `schemas/contracts`.
