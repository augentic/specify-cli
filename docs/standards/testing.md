# Testing

Integration-first test posture: `cargo nextest` over the binary, one file per integration target, golden JSON checked in. Read this before adding a new test or harness.

## Posture

Use `cargo make test` rather than `cargo test`. It runs `cargo nextest run --all --all-features --no-tests=pass` with `RUSTFLAGS=-Dwarnings` and a clean prelude, matching CI exactly.

`cargo nextest` and `cargo test` differ on `--no-tests=pass`. CI uses nextest with `--no-tests=pass`, so an empty test target is fine — but a missing `[[test]]` declaration that should exist will silently produce no output. Cross-check `cargo test` output if you suspect a target is being skipped.

## Integration-first policy

Integration tests under `tests/` use `assert_cmd::Command::cargo_bin("specrun")`, drive the binary through clap, and assert against stdout JSON or filesystem state. Test-binary names are `tests/<area>.rs` (`cache`, `cli`, `contract_tool`, `e2e`, `fan_in_fan_out`, `init`, `journal`, `plan`, `plan_orchestrate`, `registry`, `slice`, `slice_merge`, `source`, `source_preview`, `target`, `tool`, `tool_schema`, `workspace`).

One file per integration binary is the intentional layout — `tests/it.rs` consolidation was measured and dropped, see [DECISIONS.md "Integration tests"](../../DECISIONS.md#integration-tests--keep-per-file-binaries-no-testsitrs-umbrella). The cold-build win was 7.3 % cargo-reported (well below the 20 % bar we apply to "Idiomatic Rust Cleanup" chunks) and the per-binary split keeps `cargo test --test <area>` cheap for local iteration.

If a function needs unit tests, it belongs in a workspace crate, not the binary — see [architecture.md §"Workspace layout"](./architecture.md#workspace-layout) and [handler-shape.md §"Dispatcher contract"](./handler-shape.md#dispatcher-contract).

## Test naming

Test function names are identifiers, not sentences — the same brevity rules as production code ([coding-standards.md §"Naming"](./coding-standards.md#naming)) apply. The enclosing context already names the subject: an integration binary `tests/<area>.rs` supplies `<area>`, and an in-file `mod tests` (or `mod doctor`) supplies its module. Don't restate it in every `fn`.

- Drop tokens the binary name or enclosing module already supplies: in `engine_layout.rs`, write `different_skeletons_error`, not `layout_different_skeletons_is_an_error`.
- Group a cluster that shares a subject under a nested `mod <subject>` rather than repeating the subject as a prefix: six `mark_complete_*` tests become `mod mark_complete { fn idempotent() … }`.
- Compress outcome tails to the assertion's shape: `_is_an_error` / `_returns_…_error` → `_errors`; `_validates_cleanly` → `_validates`; `_surfaces_as_a_single_error_entry` → `_one_error`.
- Push the full narrative into the test body or a `//` comment above the `fn`, not the identifier.

`module_name_repetitions` does not fire on `#[test]` fns, so the dedicated `RustTestNaming` predicate enforces a 40-char cap instead. It scans an upward attribute window, so `#[tokio::test]` / `async fn` and tests behind intervening attributes (`#[ignore]`, `#[case(..)]`) are covered. `tests/rust_quality.rs::no_long_test_fn_names` fails CI on any `rust.test-fn-name-too-long` finding.

## Patterns to follow

- Spin up a real `specrun init` in a `tempfile::TempDir`. Reach for the shared helpers in `tests/common/mod.rs` (`init_workspace`, `copy_dir`, `run_git`/`GIT_ENV`) and follow the fake-forge bare-repo patterns in `tests/workspace.rs` for multi-repo / fake-forge work; do not invent a parallel harness.
- Compare stdout JSON against checked-in goldens under `tests/fixtures/e2e/goldens/`. Regenerate with `REGENERATE_GOLDENS=1 cargo nextest run --test e2e` and `git diff` before committing. The harness substitutes tempdir paths to `<TEMPDIR>` so goldens stay machine-independent.
- Prefer structural assertions (status fields, exit codes, JSON shape) over byte-for-byte prose comparisons.
- Tests that need git operations set the four `GIT_*` env vars from `tests/common::GIT_ENV` so authorship is deterministic.

`tests/fan_in_fan_out.rs` is the RM-05 (multi-repo acceptance) deterministic CLI proof — the end-to-end fan-in-twice / fan-out-once path (`source survey` → `plan propose --dry-run | --from` → per-slice `source extract` → `slice synthesize` → `slice build` → `slice merge`, plus `depends-on` ordering and byte-identical kernel re-projection). Read it first when extending multi-repo coverage; the exhaustive reconcile-code coverage over the same fan-out shape lives in `tests/plan_orchestrate/`.

## Golden file discipline

`REGENERATE_GOLDENS=1` is the single supported regeneration switch. After regenerating, run `git diff` on the goldens and review every change — a diff that updates a kebab-case error `code` field is a public-contract change (see [coding-standards.md §"Errors"](./coding-standards.md#errors) and [DECISIONS.md §"Wire compatibility"](../../DECISIONS.md#wire-compatibility)).

## Test-side gotchas

- Never hand-edit `.metadata.yaml` from a test or fixture. Drive transitions through `specrun slice transition`, `specrun plan transition`, or `stamp_slice_outcome` in `tests/common/mod.rs` when a test needs a stamped phase outcome. The tests in `tests/slice.rs` are the canonical patterns.
- WASI fixture components used by `tests/tool.rs` are rebuilt via `scripts/regen-wasm-fixtures.sh`. The outputs are checked in; only re-run when a fixture source changes.
