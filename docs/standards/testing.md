# Testing

Integration-first test posture: `cargo nextest` over the binary, one file per integration target, golden JSON checked in. Read this before adding a new test or harness.

## Posture

Use `cargo make test` rather than `cargo test`. It runs `cargo nextest run --all --all-features --no-tests=pass` with `RUSTFLAGS=-Dwarnings` and a clean prelude, matching CI exactly.

`cargo nextest` and `cargo test` differ on `--no-tests=pass`. CI uses nextest with `--no-tests=pass`, so an empty test target is fine — but a missing `[[test]]` declaration that should exist will silently produce no output. Cross-check `cargo test` output if you suspect a target is being skipped.

## Integration-first policy

Integration tests under `tests/` use `assert_cmd::Command::cargo_bin("specify")`, drive the binary through clap, and assert against stdout JSON or filesystem state. Test-binary names are `tests/<area>.rs` (`change_umbrella`, `cli`, `contract_tool`, `cross_repo`, `e2e`, `plan`, `slice`, `slice_merge`, `tool`, `vectis_tool`, `capability`).

One file per integration binary is the intentional layout — `tests/it.rs` consolidation was measured and dropped, see [DECISIONS.md "Integration tests"](../../DECISIONS.md#integration-tests--keep-per-file-binaries-no-testsitrs-umbrella). The cold-build win was 7.3 % cargo-reported (well below the 20 % bar we apply to "Idiomatic Rust Cleanup" chunks) and the per-binary split keeps `cargo test --test <area>` cheap for local iteration.

If a function needs unit tests, it belongs in a workspace crate, not the binary — see [architecture.md §"Workspace layout"](./architecture.md#workspace-layout) and [handler-shape.md §"Dispatcher contract"](./handler-shape.md#dispatcher-contract).

## Patterns to follow

- Spin up a real `specify init` in a `tempfile::TempDir`. Reach for the existing helpers in `tests/cross_repo.rs` for multi-repo / fake-forge work; do not invent a parallel harness.
- Compare stdout JSON against checked-in goldens under `tests/fixtures/e2e/goldens/`. Regenerate with `REGENERATE_GOLDENS=1 cargo nextest run --test e2e` and `git diff` before committing. The harness substitutes tempdir paths to `<TEMPDIR>` so goldens stay machine-independent.
- Prefer structural assertions (status fields, exit codes, JSON shape) over byte-for-byte prose comparisons.
- Tests that need git operations set the four `GIT_*` env vars from `tests/common::GIT_ENV` so authorship is deterministic.

`tests/cross_repo.rs` is the RM-01 happy-path acceptance harness — read it first when extending multi-repo coverage.

## Golden file discipline

`REGENERATE_GOLDENS=1` is the single supported regeneration switch. After regenerating, run `git diff` on the goldens and review every change — a diff that updates a kebab-case error `code` field is a public-contract change (see [coding-standards.md §"Errors"](./coding-standards.md#errors) and [DECISIONS.md §"Wire compatibility"](../../DECISIONS.md#wire-compatibility)) and may require bumping `ENVELOPE_VERSION`.

## Test-side gotchas

- Never hand-edit `.metadata.yaml` from a test or fixture. Drive transitions through `specify slice transition`, `specify slice outcome set`, `specify change plan transition`. The tests in `tests/slice.rs` are the canonical patterns.
- WASI fixture components used by `tests/tool.rs` are rebuilt via `scripts/regen-wasm-fixtures.sh`. The outputs are checked in; only re-run when a fixture source changes.
