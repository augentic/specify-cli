# Specify CLI Schemas

This directory contains JSON Schemas and bundled workflow fixtures used by the `specify` CLI.

## CLI-owned schemas

| Schema | Purpose |
|---|---|
| [`capability.schema.json`](capability.schema.json) | Validates a Specify capability manifest (still on disk as `schema.yaml` until the loud rename in chunk 1.6). |
| [`brief/schema.json`](brief/schema.json) | Validates YAML frontmatter in schema brief markdown files. |
| [`plan/plan.schema.json`](plan/plan.schema.json) | Validates `plan.yaml` structure. |
| [`plan-validate-output/schema.json`](plan-validate-output/schema.json) | Validates `specify plan validate --format json` output. |
| [`cache-meta.schema.json`](cache-meta.schema.json) | Validates schema cache metadata written under `.specify/.cache`. |

## Bundled workflow schema

The CLI also carries a small [`omnia`](omnia/README.md) workflow schema fixture for tests and examples. The published Specify workflow schemas live in `augentic/specify` under `schemas/omnia`, `schemas/vectis`, and `schemas/contracts`.
