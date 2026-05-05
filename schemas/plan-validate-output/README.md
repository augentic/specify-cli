# `plan-validate-output/schema.json`

Canonical JSON Schema (2020-12) for the response body emitted by `specify plan validate --format json` under the v2 JSON contract.

## Producer

`specify plan validate --format json` emits an object shaped like:

```json
{
  "schema-version": 2,
  "plan": {
    "name": "<kebab-initiative-name>",
    "path": "<absolute-or-relative-path-to-plan.yaml>"
  },
  "results": [
    {
      "level": "error | warning",
      "code": "<stable-identifier>",
      "entry": "<plan-entry-name> | null",
      "message": "<human readable>"
    }
  ],
  "passed": true
}
```

`passed` is `true` when no `error`-level finding is present; warnings do not flip it. `results` is emitted as an array even when empty. The exit code is `0` when `passed` is `true`, and `2` (`EXIT_VALIDATION_FAILED`) otherwise.

## Consumer wiring

Skills that shell out to `specify plan validate --format json` should parse the response against this schema before branching on `results`. The recommended pattern in a Node- or Python-driven runner is to pin the schema via the checked-in file path rather than fetching it at runtime so validation stays hermetic.

The same `schema.json` is the source of truth for Rust-side CLI tests (`tests/plan.rs` under `specify-cli`); treat that file as the canonical consumer when patching the schema.

## RFC-3b validation codes

`specify plan validate` emits four additional codes when `registry.yaml` is present:

- `project-not-in-registry` (error): a change's `project` value does not match any `projects[].name` in the registry.
- `project-missing-multi-repo` (error): when the registry has multiple projects, a change is missing the required `project` field.
- `description-missing-multi-repo` (error): when the registry has multiple projects, a project entry is missing the required `description` field.
- `schema-mismatch-workspace` (warning): a workspace clone's `.specify/project.yaml` declares a different `schema` than the corresponding registry entry.

## See also

- [`../plan/README.md`](../plan/README.md) - companion schema for the on-disk `plan.yaml` file this command validates.
- [`../plan/plan.schema.json`](../plan/plan.schema.json) - structural schema for the input; findings like `duplicate-name` and `dependency-cycle` reported here layer semantic checks on top.
