# `plan-validate-output/schema.json`

Canonical JSON Schema (2020-12) for the response body emitted by `specrun plan validate --format json`.

## Producer

`specrun plan validate --format json` emits an object shaped like:

```json
{
  "plan": {
    "name": "<kebab-change-name>",
    "path": "<absolute-or-relative-path-to-plan.yaml>"
  },
  "results": [
    {
      "severity": "error | warning",
      "code": "<stable-identifier>",
      "entry": "<plan-entry-name> | null",
      "message": "<human readable>",
      "data": { "kind": "cycle | orphan-source | stale-clone", "...": "..." }
    }
  ],
  "passed": true
}
```

`passed` is `true` when no `error`-level finding is present; warnings do not flip it. `results` is emitted as an array even when empty. The exit code is `0` when `passed` is `true`, and `2` (`EXIT_VALIDATION_FAILED`) otherwise.

## Consumer wiring

Skills that shell out to `specrun plan validate --format json` should parse the response against this schema before branching on `results`. The recommended pattern in a Node- or Python-driven runner is to pin the schema via the checked-in file path rather than fetching it at runtime so validation stays hermetic.

The same `schema.json` is the source of truth for Rust-side CLI tests (`tests/plan.rs` under `specify-cli`); treat that file as the canonical consumer when patching the schema.

## Validation codes

`specrun plan validate` emits additional codes when `registry.yaml` is present:

- `project-not-in-registry` (error): a slice's `project` value does not match any `projects[].name` in the registry.
- `topology-cache-stale` (warning): a workspace slot's `.specify/project.yaml` (target adapter, description) or baseline projection (`surface[]`, `decisions[]`, `recent[]`) has drifted from the committed `.specify/topology.lock`. Per [RFC-36](https://github.com/augentic/specify/blob/main/rfcs/rfc-36-project-identity.md) the slot's `project.yaml` is authoritative; the fix is `specrun workspace sync` to regenerate the cache. Replaces the former registry-authored `adapter-mismatch-workspace` / `description-missing-multi-repo` checks.
- `workspace-slot-config-unreadable` (error): a materialised slot's `project.yaml` could not be loaded or its target adapter could not be resolved.

The three health diagnostics layer additional codes that carry an optional `data` payload describing the offending shape:

- `cycle-in-depends-on` (error): one or more cycles in the `depends-on` graph; `data.kind` is `cycle` and `data.cycle` is the cycle path with the first node repeated at the end.
- `orphan-source` (warning): a top-level `sources:` key that no entry references; `data.kind` is `orphan-source` and `data.key` is the unreferenced key.
- `stale-workspace-clone` (warning): a registry-backed `.specify/workspace/<project>/` slot whose materialisation no longer matches `registry.yaml`; `data.kind` is `stale-clone` with `data.project`, `data.reason`, and optional `data.expected` / `data.observed` signature snapshots.

## See also

- [`../plan/README.md`](../plan/README.md) - companion schema for the on-disk `plan.yaml` file this command validates.
- [`../plan/plan.schema.json`](../plan/plan.schema.json) - structural schema for the input; findings like `duplicate-name` and `cycle-in-depends-on` reported here layer semantic checks on top.
