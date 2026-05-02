# `plan.schema.json`

Canonical JSON Schema (2020-12) for `plan.yaml` (at the repo root), the initiative plan described in RFC-2.

## What it validates

- Top-level `name` (kebab-case) and `changes` (ordered list) are required.
- Optional top-level `sources` map (kebab-case keys to path-or-URL values).
- Each change carries a required kebab-case `name`, a required `status` drawn from `{pending, in-progress, done, blocked, failed, skipped}`, plus optional `project`, `schema`, `depends-on`, `sources`, `context`, `description`, and `status-reason` fields.
- `additionalProperties: false` everywhere; unknown fields are a hard error.

Scope and delta-targeting intent are carried in the `description` and `context` fields. The define skill infers extract filters and baseline targets from those fields at execution time.

Semantic checks (cycle detection, referential integrity of `depends-on` / `sources` targets, at-most-one `in-progress`, registry project checks, etc.) are performed by `Plan::validate` in `specify-change`; this schema covers shape only.

The JSON response produced by `specify plan validate --format json` is itself covered by a sibling schema at [`../plan-validate-output/schema.json`](../plan-validate-output/schema.json); skill authors consuming the validator should match the response against that schema.

## Editor integration

Add the following header to `plan.yaml` to opt in to autocomplete and diagnostics in editors with `yaml-language-server` support:

```yaml
# yaml-language-server: $schema=https://raw.githubusercontent.com/augentic/specify-cli/main/schemas/plan/plan.schema.json
```

Pin to a commit or tag by replacing `main` with the desired ref.
