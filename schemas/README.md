# Specify CLI Schemas

This directory contains JSON Schemas and bundled workflow fixtures used by the `specify` CLI.

## CLI-owned schemas

| Schema | Purpose |
|---|---|
| [`adapter.schema.json`](adapter.schema.json) | Shared shape for source and target adapter manifests (`adapter.yaml`) per workflow §Adapter implementation shape; requires `description` and pins every `tools[].version` to strict semver. |
| [`source.schema.json`](source.schema.json) | Refines `adapter.schema.json` for source adapters per workflow §Source adapter contract (axis=source; closed `briefs.keys() ⊆ {enumerate, extract}`). |
| [`target.schema.json`](target.schema.json) | Refines `adapter.schema.json` for target adapters per workflow §Target adapter contract (axis=target; closed `briefs.keys() ⊆ {shape, build, merge}`). |
| [`tool.schema.json`](tool.schema.json) | Validates the standalone WASI `tools[]` declaration block — kebab-case names, strict-semver versions, source URIs with the `$PROJECT_DIR` / `$CAPABILITY_DIR` prefixes, and `permissions.{read,write}` paths that may not target `.specify/`. |
| [`evidence.schema.json`](evidence.schema.json) | Validates per-source `Evidence` files written by `source.extract` per workflow §Source adapter contract; closed `kind` enum includes the spatial `region` / `container` / `leaf` kinds from day one. |
| [`discovery/candidate.schema.json`](discovery/candidate.schema.json) | Validates a single candidate block under `## Candidate inventory` in `discovery.md` per workflow §Discovery handshake. |
| [`plan/plan.schema.json`](plan/plan.schema.json) | Validates `plan.yaml` structure, including the structured `sources[<key>]` binding shape and the `name@vN` target suffix reconciled against the resolved adapter's `version`. |
| [`plan-validate-output/schema.json`](plan-validate-output/schema.json) | Validates `specrun plan validate --format json` output. |
| [`slice/fusion.schema.json`](slice/fusion.schema.json) | Validates a slice's `fusion.yaml` reconciliation index as the audit surface listing every `REQ-*` id with its contributing `(source, claim-id)` pairs and authority outcome. |
| [`vectis/template-manifest.schema.json`](vectis/template-manifest.schema.json) | Validates `templates/vectis/manifest.yaml`, the source-to-target assembly map consumed by `wasi-tools/vectis/build.rs`. |
| [`cache-meta.schema.json`](cache-meta.schema.json) | Validates schema cache metadata written under `.specify/.cache`. |
| [`context-lock.schema.json`](context-lock.schema.json) | Validates `.specify/context.lock`, the sidecar used by init-time AGENTS.md generation. |
| [`design-system/components.schema.json`](design-system/components.schema.json) | Validates `.specify/design-system/components.yaml`, the operator-curated component catalog. Declares shared UI components that the Vectis target factors into shared code at build time. |

## Bundled workflow schema

The published Specify workflow target adapters live in `augentic/specify` under `adapters/targets/omnia`, `adapters/targets/vectis`, and `adapters/targets/contracts`. The CLI carries a minimal [`tests/fixtures/adapters/targets/omnia/`](../tests/fixtures/adapters/targets/omnia) workflow-contract fixture for its own integration tests.
