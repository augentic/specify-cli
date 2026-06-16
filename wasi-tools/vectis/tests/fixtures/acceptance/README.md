# Acceptance fixture mirror (RFC-46 R46-S25)

Committed copy of `evals/fixtures/targets/vectis/task-list/design-system/` from the `augentic/specify` framework repo. The `wasi-tools` CI job runs golden layout checks against this tree without checking out the framework repo.

**Keep in sync:** after editing canonical masters or re-running `vectis materialize assets` on the framework fixture, copy the updated `design-system/` tree here:

```bash
cp -R ../specify/evals/fixtures/targets/vectis/task-list/design-system \
  wasi-tools/vectis/tests/fixtures/acceptance/task-list
```

Tests: `cargo test -p specify-vectis acceptance_fixture` in `wasi-tools/`.
