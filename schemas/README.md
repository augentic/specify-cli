# Specify CLI Schemas

This directory contains JSON Schemas and bundled workflow fixtures used by the `specify` CLI.

## CLI-owned schemas

| Schema | Purpose |
|---|---|
| [`adapter.schema.json`](adapter.schema.json) | Validates a Specify adapter manifest (`adapter.yaml`) per RFC-13 §Adapter manifest and protocol. (Pre-RFC-25; retained for v1.x manifests until the W0.3 loader replacement.) |
| [`plugin.schema.json`](plugin.schema.json) | Shared shape for RFC-25 source and target adapter manifests — the axis-discriminated parent schema. |
| [`source.schema.json`](source.schema.json) | Refines `plugin.schema.json` for source adapters per RFC-25 §Source adapter contract (axis=source; operations=[enumerate, extract]). |
| [`target.schema.json`](target.schema.json) | Refines `plugin.schema.json` for target adapters per RFC-25 §Target adapter contract (axis=target; operations=[shape, build, merge]). |
| [`evidence.schema.json`](evidence.schema.json) | Validates per-source `Evidence` files written by `source.extract` per RFC-25 §Source adapter contract; closed `kind` enum includes the spatial `region` / `container` / `leaf` kinds from day one. |
| [`discovery/candidate.schema.json`](discovery/candidate.schema.json) | Validates a single candidate block under `## Candidate inventory` in `discovery.md` per RFC-25 §Discovery handshake. |
| [`brief/schema.json`](brief/schema.json) | Validates YAML frontmatter in adapter brief markdown files. |
| [`codex-rule.schema.json`](codex-rule.schema.json) | Validates YAML frontmatter in codex rule markdown files. |
| [`plan/plan.schema.json`](plan/plan.schema.json) | Validates `plan.yaml` structure. |
| [`plan-validate-output/schema.json`](plan-validate-output/schema.json) | Validates `specify plan validate --format json` output. |
| [`cache-meta.schema.json`](cache-meta.schema.json) | Validates schema cache metadata written under `.specify/.cache`. |
| [`context-lock.schema.json`](context-lock.schema.json) | Validates `.specify/context.lock`, the sidecar used by init-time AGENTS.md generation. |

## Bundled workflow schema

The published Specify workflow target adapters live in `augentic/specify` under `adapters/targets/omnia`, `adapters/targets/vectis`, and `adapters/targets/contracts`. The CLI carries a minimal [`tests/fixtures/adapters/targets/omnia/`](../tests/fixtures/adapters/targets/omnia) RFC-25 fixture for its own integration tests.
